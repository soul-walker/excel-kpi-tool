//! calc：表达式 DSL（Excel 风格子集）。
//!
//! M1 第一块地基。语法要点：
//! - 数值 / 字符串（单双引号）/ 布尔；中文标识符；点分路径（`参数.课时费`、`qingjia.课时`）；
//! - `??`（或 `?`）空值回退；算术、比较、AND/OR/NOT；
//! - 分支用编程风格块语法 `if 条件 { 真 } else { 假 }`（可 `else if` 链；else 可省略得 `空`），
//!   惰性求值；旧 Excel 函数写法 `IF(c,a,b)` 已移除；
//! - 内置函数 MIN/MAX/ROUND/ABS；
//! - 聚合 `SUM/COUNT/AVG/MIN/MAX(<expr>)`：在行集合上下文求值（行内列带 `{表}.` 前缀）；
//!   表达式内**不支持 WHERE 子句**——行过滤统一由派生/聚合的 `where` 字段（SQL WHERE）承担。
//!
//! 求值依赖外部回调做"路径 → 值"解析（避免本模块与 ECS/表格耦合）。

use std::fmt;

/// 词法单元。
#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Str(String),
    Ident(String),
    Comma,
    LParen,
    RParen,
    LBrace,
    RBrace,
    Dot,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Coalesce, // ??
    Eof,
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}
fn is_ident_part(c: char) -> bool {
    // 中文字段名可能含顿号（未参加升旗、集会）与百分号（上浮30%）
    c.is_alphanumeric() || c == '_' || c == '、' || c == '%'
}

fn lex(src: &str) -> Result<Vec<Tok>, String> {
    let mut toks = Vec::new();
    let mut chars = src.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            c if c.is_whitespace() => {
                chars.next();
            }
            '\'' | '"' => {
                chars.next();
                let mut s = String::new();
                let mut closed = false;
                for ch in chars.by_ref() {
                    if ch == c {
                        closed = true;
                        break;
                    }
                    s.push(ch);
                }
                if !closed {
                    return Err("未闭合的字符串字面量".into());
                }
                toks.push(Tok::Str(s));
            }
            c if c.is_ascii_digit() => {
                let mut num = String::new();
                while let Some(&d) = chars.peek() {
                    if d.is_ascii_digit() || d == '.' {
                        num.push(d);
                        chars.next();
                    } else {
                        break;
                    }
                }
                let v: f64 = num.parse().map_err(|_| format!("非法数字: {num}"))?;
                toks.push(Tok::Num(v));
            }
            c if is_ident_start(c) => {
                let mut s = String::new();
                while let Some(&d) = chars.peek() {
                    if is_ident_part(d) {
                        s.push(d);
                        chars.next();
                    } else {
                        break;
                    }
                }
                toks.push(Tok::Ident(s));
            }
            '?' => {
                // `?` 与 `??` 均作空值回退（编程风格 `keshi.课时 ? 0`）。
                chars.next();
                if chars.peek() == Some(&'?') {
                    chars.next();
                }
                toks.push(Tok::Coalesce);
            }
            _ => {
                chars.next();
                let t = match c {
                    ',' => Tok::Comma,
                    '(' => Tok::LParen,
                    ')' => Tok::RParen,
                    '{' => Tok::LBrace,
                    '}' => Tok::RBrace,
                    '.' => Tok::Dot,
                    '+' => Tok::Plus,
                    '-' => Tok::Minus,
                    '*' => Tok::Star,
                    '/' => Tok::Slash,
                    '%' => Tok::Percent,
                    '=' => Tok::Eq,
                    '<' => {
                        if chars.peek() == Some(&'=') {
                            chars.next();
                            Tok::Le
                        } else {
                            Tok::Lt
                        }
                    }
                    '>' => {
                        if chars.peek() == Some(&'=') {
                            chars.next();
                            Tok::Ge
                        } else {
                            Tok::Gt
                        }
                    }
                    '!' => {
                        if chars.peek() == Some(&'=') {
                            chars.next();
                            Tok::Ne
                        } else {
                            return Err("仅支持 != (不应使用 !)".into());
                        }
                    }
                    other => return Err(format!("无法识别的字符: {other}")),
                };
                toks.push(t);
            }
        }
    }
    toks.push(Tok::Eof);
    Ok(toks)
}

/// AST。
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Num(f64),
    Str(String),
    Bool(bool),
    /// 空字面量（`空`/`null`/`empty`），求值得 `Value::Empty`。
    Empty,
    /// 点分路径，如 `参数.课时费`、`课时.课时`、`请假超额`。
    Path(Vec<String>),
    If {
        cond: Box<Node>,
        then: Box<Node>,
        els: Box<Node>,
    },
    Call {
        name: String,
        args: Vec<Node>,
    },
    /// 聚合（在行集合上下文按行取值后归约）。
    Agg {
        op: String,
        expr: Box<Node>,
    },
    Bin {
        op: BinOp,
        l: Box<Node>,
        r: Box<Node>,
    },
    Not(Box<Node>),
    Coalesce {
        l: Box<Node>,
        r: Box<Node>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

/// 解析完整表达式。
pub fn parse(src: &str) -> Result<Node, String> {
    let toks = lex(src)?;
    let mut p = Parser { toks, pos: 0 };
    let n = p.parse_or()?;
    if !matches!(p.peek(), Tok::Eof) {
        return Err(format!("表达式末尾存在多余内容: {:?}", p.peek()));
    }
    Ok(n)
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos.min(self.toks.len() - 1)]
    }
    fn next(&mut self) -> Tok {
        let t = self.peek().clone();
        if self.pos < self.toks.len() - 1 {
            self.pos += 1;
        }
        t
    }
    fn eat(&mut self, t: &Tok) -> Result<(), String> {
        if self.peek() == t {
            self.next();
            Ok(())
        } else {
            Err(format!("期望 {:?}，实际 {:?}", t, self.peek()))
        }
    }

    fn parse_or(&mut self) -> Result<Node, String> {
        let mut l = self.parse_and()?;
        while let Tok::Ident(kw) = self.peek() {
            if kw.eq_ignore_ascii_case("or") {
                self.next();
                let r = self.parse_and()?;
                l = Node::Bin {
                    op: BinOp::Or,
                    l: Box::new(l),
                    r: Box::new(r),
                };
            } else {
                break;
            }
        }
        Ok(l)
    }

    fn parse_and(&mut self) -> Result<Node, String> {
        let mut l = self.parse_not()?;
        while let Tok::Ident(kw) = self.peek() {
            if kw.eq_ignore_ascii_case("and") {
                self.next();
                let r = self.parse_not()?;
                l = Node::Bin {
                    op: BinOp::And,
                    l: Box::new(l),
                    r: Box::new(r),
                };
            } else {
                break;
            }
        }
        Ok(l)
    }

    fn parse_not(&mut self) -> Result<Node, String> {
        if let Tok::Ident(kw) = self.peek()
            && kw.eq_ignore_ascii_case("not")
        {
            self.next();
            let inner = self.parse_not()?;
            return Ok(Node::Not(Box::new(inner)));
        }
        self.parse_coalesce()
    }

    fn parse_coalesce(&mut self) -> Result<Node, String> {
        let l = self.parse_compare()?;
        if self.peek() == &Tok::Coalesce {
            self.next();
            let r = self.parse_coalesce()?;
            return Ok(Node::Coalesce {
                l: Box::new(l),
                r: Box::new(r),
            });
        }
        Ok(l)
    }

    fn parse_compare(&mut self) -> Result<Node, String> {
        let l = self.parse_add()?;
        let op = match self.peek() {
            Tok::Eq => BinOp::Eq,
            Tok::Ne => BinOp::Ne,
            Tok::Lt => BinOp::Lt,
            Tok::Le => BinOp::Le,
            Tok::Gt => BinOp::Gt,
            Tok::Ge => BinOp::Ge,
            _ => return Ok(l),
        };
        self.next();
        let r = self.parse_add()?;
        Ok(Node::Bin {
            op,
            l: Box::new(l),
            r: Box::new(r),
        })
    }

    fn parse_add(&mut self) -> Result<Node, String> {
        let mut l = self.parse_mul()?;
        loop {
            let op = match self.peek() {
                Tok::Plus => BinOp::Add,
                Tok::Minus => BinOp::Sub,
                _ => break,
            };
            self.next();
            let r = self.parse_mul()?;
            l = Node::Bin {
                op,
                l: Box::new(l),
                r: Box::new(r),
            };
        }
        Ok(l)
    }

    fn parse_mul(&mut self) -> Result<Node, String> {
        let mut l = self.parse_unary()?;
        loop {
            let op = match self.peek() {
                Tok::Star => BinOp::Mul,
                Tok::Slash => BinOp::Div,
                Tok::Percent => BinOp::Mod,
                _ => break,
            };
            self.next();
            let r = self.parse_unary()?;
            l = Node::Bin {
                op,
                l: Box::new(l),
                r: Box::new(r),
            };
        }
        Ok(l)
    }

    fn parse_unary(&mut self) -> Result<Node, String> {
        if self.peek() == &Tok::Minus {
            self.next();
            let inner = self.parse_unary()?;
            return Ok(Node::Bin {
                op: BinOp::Sub,
                l: Box::new(Node::Num(0.0)),
                r: Box::new(inner),
            });
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<Node, String> {
        match self.next() {
            Tok::Num(v) => Ok(Node::Num(v)),
            Tok::Str(s) => Ok(Node::Str(s)),
            Tok::LParen => {
                let inner = self.parse_or()?;
                self.eat(&Tok::RParen)?;
                Ok(inner)
            }
            Tok::Ident(first) => {
                if first.eq_ignore_ascii_case("true") {
                    return Ok(Node::Bool(true));
                }
                if first.eq_ignore_ascii_case("false") {
                    return Ok(Node::Bool(false));
                }
                if first == "空"
                    || first.eq_ignore_ascii_case("null")
                    || first.eq_ignore_ascii_case("empty")
                {
                    return Ok(Node::Empty);
                }
                // `if` 关键字：只支持块语法 `if 条件 { 真 } else { 假 }`。
                // `if (` 后括号内含顶层逗号即旧 Excel 函数写法 `IF(c,a,b)`，给迁移提示；
                // 否则按块语法解析（括号条件 `if (cond) { … }` 也允许）。
                if first.eq_ignore_ascii_case("if") {
                    if self.peek() == &Tok::LParen && self.looks_like_legacy_if() {
                        return Err(
                            "已移除 Excel 函数写法 IF(c,a,b)；请改用块语法：if 条件 { 真 } else { 假 }"
                                .into(),
                        );
                    }
                    return self.parse_if_chain();
                }
                // 点分路径：a.b.c
                let mut parts = vec![first];
                while self.peek() == &Tok::Dot {
                    self.next();
                    if let Tok::Ident(seg) = self.next() {
                        parts.push(seg);
                    } else {
                        return Err("路径中 '.' 后应为标识符".into());
                    }
                }
                // 函数 / 聚合调用
                if self.peek() == &Tok::LParen {
                    self.next();
                    let name = parts.join(".");
                    // 零参函数，如 LEAVE_PENALTY()
                    if self.peek() == &Tok::RParen {
                        self.next();
                        return Ok(Node::Call {
                            name,
                            args: Vec::new(),
                        });
                    }
                    let agg_like = ["sum", "count", "avg", "min", "max"];
                    // 先解析第一个参数
                    let first = self.parse_or()?;
                    if let Tok::Ident(kw) = self.peek()
                        && kw.eq_ignore_ascii_case("where")
                    {
                        return Err(
                            "表达式内不再支持 WHERE 子句；请把行过滤写到派生/聚合的 where 字段"
                                .into(),
                        );
                    }
                    if self.peek() == &Tok::Comma {
                        // 多参普通函数，如 MAX(0, x)
                        let mut args = vec![first];
                        while self.peek() == &Tok::Comma {
                            self.next();
                            args.push(self.parse_or()?);
                        }
                        self.eat(&Tok::RParen)?;
                        return Ok(Node::Call { name, args });
                    }
                    self.eat(&Tok::RParen)?;
                    if agg_like.iter().any(|a| a.eq_ignore_ascii_case(&name)) {
                        return Ok(Node::Agg {
                            op: name.to_lowercase(),
                            expr: Box::new(first),
                        });
                    }
                    return Ok(Node::Call {
                        name,
                        args: vec![first],
                    });
                }
                Ok(Node::Path(parts))
            }
            other => Err(format!("意外的单元: {:?}", other)),
        }
    }

    /// `if (` 是否为旧 Excel 函数写法 `IF(c,a,b)`：括号内存在顶层（相对该括号）
    /// 逗号即判定为旧写法（DSL 表达式里逗号只会作函数参数分隔，条件本身不含顶层逗号）。
    fn looks_like_legacy_if(&self) -> bool {
        let mut depth = 0usize;
        let mut found_comma = false;
        for t in self.toks.iter().skip(self.pos) {
            match t {
                Tok::LParen => depth += 1,
                Tok::RParen => {
                    depth -= 1;
                    if depth == 0 {
                        return found_comma;
                    }
                }
                Tok::Comma if depth == 1 => found_comma = true,
                _ => {}
            }
        }
        // 流未闭合：按已见结果判断，具体语法错误交由后续解析报出。
        found_comma
    }

    /// 解析 `if` 关键字之后的整条链（当前 token 为 `if`，已由 parse_primary 消费）：
    /// `if 条件 { 真 } [else { 假 } | else if …]`。else 可省略，缺省分支得 `空`。
    fn parse_if_chain(&mut self) -> Result<Node, String> {
        let cond = self.parse_or()?;
        let then = self.parse_block()?;
        let els = if self.is_kw("else") {
            self.next();
            if self.is_kw("if") {
                self.next();
                self.parse_if_chain()?
            } else {
                self.parse_block()?
            }
        } else {
            Node::Empty
        };
        Ok(Node::If {
            cond: Box::new(cond),
            then: Box::new(then),
            els: Box::new(els),
        })
    }

    /// `{ 表达式 }` 块：按完整表达式解析花括号内内容。
    fn parse_block(&mut self) -> Result<Node, String> {
        self.eat(&Tok::LBrace)?;
        let inner = self.parse_or()?;
        self.eat(&Tok::RBrace)?;
        Ok(inner)
    }

    fn is_kw(&self, kw: &str) -> bool {
        matches!(self.peek(), Tok::Ident(k) if k.eq_ignore_ascii_case(kw))
    }
}

// ---- 求值 ----

/// 求值过程中的值（Empty ≈ 单元格空/缺值，用于 `??` 与空值语义）。
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Num(f64),
    Bool(bool),
    Str(String),
    Empty,
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Num(v) => {
                if v.fract() == 0.0 {
                    write!(f, "{}", *v as i64)
                } else {
                    write!(f, "{v}")
                }
            }
            Value::Bool(b) => write!(f, "{b}"),
            Value::Str(s) => write!(f, "{s}"),
            Value::Empty => write!(f, "∅"),
        }
    }
}

/// 求值回调：把点分路径解析为值；返回 None 表示"未知/缺失"。
pub type Lookup<'a> = &'a dyn Fn(&[String]) -> Option<Value>;

/// 额外函数回调：名称 + 已求值实参 → 结果。返回 Ok(None) 表示不认识该函数。
/// 通用扩展点：供上层（GUI 插件 / 特殊场景）注入自定义函数；当前引擎内置实现不依赖它。
pub type ExtraFunc<'a> = Option<&'a dyn Fn(&str, &[Value]) -> Result<Option<Value>, String>>;

/// 行：聚合求值用（列 alias -> 值）。
pub type Row = std::collections::HashMap<String, Value>;

/// 无行上下文求值（字段/参数表达式）。
pub fn eval(node: &Node, lookup: Lookup) -> Result<Value, String> {
    eval_core(node, lookup, None, None, None)
}

/// 带额外函数库的无行上下文求值（通用扩展点，当前无内置调用方）。
pub fn eval_with(node: &Node, lookup: Lookup, extra: ExtraFunc<'_>) -> Result<Value, String> {
    eval_core(node, lookup, None, None, extra)
}

/// 带行集合求值：其中 `SUM/COUNT(...)` 等聚合会在这些行上求值。
pub fn eval_rows(node: &Node, lookup: Lookup, rows: &[Row]) -> Result<Value, String> {
    eval_core(node, lookup, Some(rows), None, None)
}

/// 在单行作用域内求值：顶层标识符优先解析为该行的列（行过滤/行字段）。
pub fn eval_on_row(node: &Node, lookup: Lookup, row: &Row) -> Result<Value, String> {
    eval_core(node, lookup, None, Some(row), None)
}

fn resolve_path(lookup: Lookup, row: Option<&Row>, p: &[String]) -> Value {
    if let Some(r) = row
        && p.len() == 1
        && let Some(v) = r.get(&p[0])
    {
        return v.clone();
    }
    lookup(p).unwrap_or(Value::Empty)
}

fn eval_core(
    node: &Node,
    lookup: Lookup,
    rows: Option<&[Row]>,
    row: Option<&Row>,
    extra: ExtraFunc<'_>,
) -> Result<Value, String> {
    match node {
        Node::Num(v) => Ok(Value::Num(*v)),
        Node::Str(s) => Ok(Value::Str(s.clone())),
        Node::Bool(b) => Ok(Value::Bool(*b)),
        Node::Empty => Ok(Value::Empty),
        Node::Path(p) => Ok(resolve_path(lookup, row, p)),
        Node::Not(x) => Ok(Value::Bool(!as_bool(&eval_core(
            x, lookup, rows, row, extra,
        )?)?)),
        Node::Coalesce { l, r } => {
            let lv = eval_core(l, lookup, rows, row, extra)?;
            if lv != Value::Empty {
                Ok(lv)
            } else {
                eval_core(r, lookup, rows, row, extra)
            }
        }
        Node::If { cond, then, els } => {
            if as_bool(&eval_core(cond, lookup, rows, row, extra)?)? {
                eval_core(then, lookup, rows, row, extra)
            } else {
                eval_core(els, lookup, rows, row, extra)
            }
        }
        Node::Bin { op, l, r } => {
            let lv = eval_core(l, lookup, rows, row, extra)?;
            let rv = eval_core(r, lookup, rows, row, extra)?;
            eval_bin(*op, &lv, &rv)
        }
        Node::Call { name, args } => {
            let mut vals = Vec::with_capacity(args.len());
            for a in args {
                vals.push(eval_core(a, lookup, rows, row, extra)?);
            }
            let lc = name.to_lowercase();
            let plain = ["min", "max", "round", "abs"];
            if plain.iter().any(|k| *k == lc) {
                eval_call(name, &vals)
            } else if let Some(f) = extra
                && let Some(v) = f(name, &vals)?
            {
                Ok(v)
            } else {
                eval_call(name, &vals)
            }
        }
        Node::Agg { op, expr } => {
            let list = rows.ok_or_else(|| "聚合表达式缺少行集合上下文".to_string())?;
            let mut nums = Vec::new();
            let mut count = 0usize;
            for r in list {
                count += 1;
                if op != "count" {
                    let v = eval_core(expr, lookup, None, Some(r), extra)?;
                    nums.push(to_num(&v)?);
                }
            }
            match op.as_str() {
                "count" => Ok(Value::Num(count as f64)),
                "sum" => Ok(Value::Num(nums.iter().sum())),
                "avg" => Ok(Value::Num(if nums.is_empty() {
                    0.0
                } else {
                    nums.iter().sum::<f64>() / nums.len() as f64
                })),
                "min" | "max" => {
                    if nums.is_empty() {
                        Ok(Value::Empty)
                    } else if op == "min" {
                        Ok(Value::Num(nums.into_iter().fold(f64::INFINITY, f64::min)))
                    } else {
                        Ok(Value::Num(
                            nums.into_iter().fold(f64::NEG_INFINITY, f64::max),
                        ))
                    }
                }
                other => Err(format!("未知聚合操作: {other}")),
            }
        }
    }
}

fn as_bool(v: &Value) -> Result<bool, String> {
    match v {
        Value::Bool(b) => Ok(*b),
        Value::Num(n) => Ok(*n != 0.0),
        Value::Str(s) => Ok(!s.is_empty()),
        Value::Empty => Ok(false),
    }
}

fn to_num(v: &Value) -> Result<f64, String> {
    match v {
        Value::Num(n) => Ok(*n),
        Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
        Value::Empty => Ok(0.0),
        Value::Str(s) => s
            .trim()
            .parse()
            .map_err(|_| format!("无法把字符串 {s:?} 当数值")),
    }
}

fn eval_bin(op: BinOp, l: &Value, r: &Value) -> Result<Value, String> {
    use BinOp::*;
    Ok(match op {
        Add => Value::Num(to_num(l)? + to_num(r)?),
        Sub => Value::Num(to_num(l)? - to_num(r)?),
        Mul => Value::Num(to_num(l)? * to_num(r)?),
        Div => Value::Num(to_num(l)? / to_num(r)?),
        Mod => Value::Num(to_num(l)? % to_num(r)?),
        Eq => Value::Bool(l == r),
        Ne => Value::Bool(l != r),
        Lt => Value::Bool(to_num(l)? < to_num(r)?),
        Le => Value::Bool(to_num(l)? <= to_num(r)?),
        Gt => Value::Bool(to_num(l)? > to_num(r)?),
        Ge => Value::Bool(to_num(l)? >= to_num(r)?),
        And => Value::Bool(as_bool(l)? && as_bool(r)?),
        Or => Value::Bool(as_bool(l)? || as_bool(r)?),
    })
}

fn eval_call(name: &str, vals: &[Value]) -> Result<Value, String> {
    let lc = name.to_lowercase();
    match lc.as_str() {
        "min" | "max" => {
            if vals.is_empty() {
                return Err("MIN/MAX 需要至少一个参数".into());
            }
            let nums: Vec<f64> = vals.iter().map(to_num).collect::<Result<_, _>>()?;
            let v = if lc == "min" {
                nums.into_iter().fold(f64::INFINITY, f64::min)
            } else {
                nums.into_iter().fold(f64::NEG_INFINITY, f64::max)
            };
            Ok(Value::Num(v))
        }
        "round" => {
            if vals.is_empty() {
                return Err("ROUND 缺少参数".into());
            }
            let v = to_num(&vals[0])?;
            let digits = if vals.len() > 1 {
                to_num(&vals[1])? as i32
            } else {
                0
            };
            let f = 10f64.powi(digits);
            Ok(Value::Num((v * f).round() / f))
        }
        "abs" => Ok(Value::Num(to_num(&vals[0])?.abs())),
        _ => Err(format!("未知函数: {name}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(src: &str) -> Value {
        let n = parse(src).expect("解析失败");
        eval(&n, &|_| None).expect("求值失败")
    }

    #[test]
    fn precedence() {
        assert_eq!(ev("1 + 2 * 3"), Value::Num(7.0));
        assert_eq!(ev("(1 + 2) * 3"), Value::Num(9.0));
        assert_eq!(ev("10 / 4"), Value::Num(2.5));
        assert_eq!(ev("-3 + 1"), Value::Num(-2.0));
    }

    #[test]
    fn comparisons_and_logic() {
        assert_eq!(ev("1 < 2 AND 3 >= 3"), Value::Bool(true));
        assert_eq!(ev("NOT (1 = 2)"), Value::Bool(true));
        assert_eq!(ev("2 != 2 OR true"), Value::Bool(true));
    }

    #[test]
    fn if_is_lazy() {
        assert_eq!(ev("if true { 1 } else { 2 }"), Value::Num(1.0));
        assert_eq!(ev("if false { 1 } else { 2 }"), Value::Num(2.0));
        // 惰性：未走到的分支不求值（若被求值会因 ABS('abc') 报错）。
        assert_eq!(ev("if false { ABS('abc') } else { 1 }"), Value::Num(1.0));
        assert_eq!(ev("if true { 2 } else { ABS('abc') }"), Value::Num(2.0));
    }

    #[test]
    fn block_style_if() {
        // 块语法惰性求值。
        assert_eq!(ev("if true { 1 } else { 2 }"), Value::Num(1.0));
        assert_eq!(ev("if false { 1 } else { 2 }"), Value::Num(2.0));
        // else if 链
        assert_eq!(
            ev("if false { 1 } else if true { 2 } else { 3 }"),
            Value::Num(2.0)
        );
        assert_eq!(
            ev("if false { 1 } else if false { 2 } else { 3 }"),
            Value::Num(3.0)
        );
        // 括号条件
        assert_eq!(ev("if (1 < 2) { 7 } else { 8 }"), Value::Num(7.0));
        // 分支内部继续做算术 / 嵌套 if
        assert_eq!(ev("if false { 1 + 2 } else { 10 - 4 }"), Value::Num(6.0));
        assert_eq!(
            ev("if true { if false { 1 } else { 2 } } else { 3 }"),
            Value::Num(2.0)
        );
        // 无 else：条件不成立得 空
        assert_eq!(ev("if false { 1 }"), Value::Empty);
        assert_eq!(ev("if true { 1 }"), Value::Num(1.0));
    }

    #[test]
    fn legacy_if_function_form_rejected() {
        // 旧 Excel 函数写法已移除：解析即报错，且给出块语法迁移提示。
        let err = parse("IF(聚合.请假超额, 0, keshi.课时 ?? 0)").unwrap_err();
        assert!(err.contains("块语法"), "报错应提示迁移: {err}");
        assert!(parse("IF(true, 1, 2)").is_err());

        // 多行块写法（对标方案里的样例）应可解析。
        parse("if 聚合.请假超额 {\n  0\n} else {\n  keshi.课时 ? 0\n}")
            .expect("多行块语法应可解析");
    }

    #[test]
    fn functions() {
        assert_eq!(ev("MIN(3, 1, 2)"), Value::Num(1.0));
        assert_eq!(ev("MAX(3, 1, 2)"), Value::Num(3.0));
        assert_eq!(ev("ROUND(1.26, 1)"), Value::Num(1.3));
        assert_eq!(ev("ABS(-4)"), Value::Num(4.0));
    }

    #[test]
    fn not_and_brace_if_with_coalesce() {
        // 旧式嵌套 IF 的新写法要素：NOT + and + 单 `?` 空值回退 + 无 else 分支（缺省得 空）。
        // 语义与旧写法 IF(超额, 空, IF(x ?? 0 > 0, x ?? 0, 空)) 等价。
        let src = "if NOT 请假超额 and gangwei.教研组长?0 > 0 { gangwei.教研组长?0 }";
        let n = parse(src).expect("新写法应可解析");
        let run = |v: &[(&[&str], Value)]| {
            eval(&n, &|p| {
                v.iter()
                    .find(|(path, _)| *path == p)
                    .map(|(_, val)| val.clone())
            })
            .expect("求值失败")
        };
        // 超额 → 空
        assert_eq!(run(&[(&["请假超额"], Value::Bool(true))]), Value::Empty);
        // 不超额但列缺失 → 空
        assert_eq!(run(&[]), Value::Empty);
        // 不超额且值为 0 → 空
        assert_eq!(
            run(&[(&["gangwei", "教研组长"], Value::Num(0.0))]),
            Value::Empty
        );
        // 不超额且值 > 0 → 返回值
        assert_eq!(
            run(&[(&["gangwei", "教研组长"], Value::Num(5.0))]),
            Value::Num(5.0)
        );
    }

    #[test]
    fn coalesce_and_missing_path() {
        assert_eq!(ev("课时.课时 ?? 0"), Value::Num(0.0));
        assert_eq!(ev("(if true { 1 } else { 2 }) ?? 9"), Value::Num(1.0));
        // 单 `?` 与 `??` 等价。
        assert_eq!(ev("keshi.课时 ? 0"), Value::Num(0.0));
    }

    #[test]
    fn empty_literal() {
        assert_eq!(ev("空"), Value::Empty);
        assert_eq!(ev("null"), Value::Empty);
        assert_eq!(ev("if false { 1 } else { 空 }"), Value::Empty);
        // 算术中 空 按 0 处理
        assert_eq!(ev("空 + 5"), Value::Num(5.0));
    }

    #[test]
    fn chinese_identifiers_and_params() {
        assert_eq!(
            ev("if 请假超额 { 0 } else { 课时.课时 ?? 0 }"),
            Value::Num(0.0)
        );
        assert_eq!(ev("1.5 * 2"), Value::Num(3.0));
        // 参数路径存在时的取值走回调
        let n = parse("参数.基础课时 + 1").unwrap();
        let v = eval(&n, &|p| {
            if p == ["参数".to_string(), "基础课时".to_string()] {
                Some(Value::Num(15.6))
            } else {
                None
            }
        })
        .unwrap();
        assert_eq!(v, Value::Num(16.6));
    }

    #[test]
    fn aggregate_parses() {
        // 行集聚合：表达式内不再支持 WHERE 子句（行过滤由派生/聚合的 where 字段承担）。
        parse("SUM(天数)").expect("聚合表达式应可解析");
        parse("MAX(0, 课时S - 参数.基础课时)").expect("普通调用应可解析");
        // 内联 WHERE 语法已移除：`SUM(x WHERE c)` 应解析失败。
        assert!(parse("SUM(天数 WHERE 类型 != '公')").is_err());
    }

    #[test]
    fn aggregate_row_eval() {
        fn row(days: f64) -> Row {
            let mut m = Row::new();
            m.insert("天数".to_string(), Value::Num(days));
            m
        }
        let rows = vec![row(2.0), row(3.0), row(1.0)];

        let n = parse("SUM(天数)").unwrap();
        assert_eq!(eval_rows(&n, &|_| None, &rows).unwrap(), Value::Num(6.0));

        let n = parse("COUNT(天数)").unwrap();
        assert_eq!(eval_rows(&n, &|_| None, &rows).unwrap(), Value::Num(3.0));

        let n = parse("AVG(天数)").unwrap();
        assert_eq!(eval_rows(&n, &|_| None, &rows).unwrap(), Value::Num(2.0));

        let n = parse("MIN(天数)").unwrap();
        assert_eq!(eval_rows(&n, &|_| None, &rows).unwrap(), Value::Num(1.0));
        let n = parse("MAX(天数)").unwrap();
        assert_eq!(eval_rows(&n, &|_| None, &rows).unwrap(), Value::Num(3.0));

        // 空行集：SUM=0、COUNT=0；MIN/MAX=∅
        let empty: Vec<Row> = Vec::new();
        let n = parse("SUM(天数)").unwrap();
        assert_eq!(eval_rows(&n, &|_| None, &empty).unwrap(), Value::Num(0.0));
        let n = parse("MIN(天数)").unwrap();
        assert_eq!(eval_rows(&n, &|_| None, &empty).unwrap(), Value::Empty);
    }

    #[test]
    fn parse_errors() {
        assert!(parse("1 +").is_err());
        assert!(parse("(1 + 2").is_err());
        assert!(parse("'abc").is_err());
    }
}
