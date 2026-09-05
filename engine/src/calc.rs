//! calc：表达式 DSL（Excel 风格子集）。
//!
//! M1 第一块地基。语法要点：
//! - 数值 / 字符串（单双引号）/ 布尔；中文标识符；点分路径（`参数.课时费`、`课时.课时`）；
//! - `??` 空值回退；算术、比较、AND/OR/NOT；
//! - `IF(c,a,b)` 惰性三参；内置函数 MIN/MAX/ROUND/ABS；
//! - 聚合写法 `SUM(<expr> WHERE <cond>)`（本阶段完成解析，行作用域求值见 M1 1.1-待办）。
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
    c.is_alphanumeric() || c == '_'
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
                chars.next();
                match chars.next() {
                    Some('?') => toks.push(Tok::Coalesce),
                    _ => return Err("仅支持 ?? (应为两个问号)".into()),
                }
            }
            _ => {
                chars.next();
                let t = match c {
                    ',' => Tok::Comma,
                    '(' => Tok::LParen,
                    ')' => Tok::RParen,
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
    /// `SUM(expr WHERE cond)` 等聚合（解析完成；行作用域求值后续实现）。
    Agg {
        op: String,
        expr: Box<Node>,
        cond: Option<Box<Node>>,
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
                // 函数 / 聚合 / IF 调用
                if self.peek() == &Tok::LParen {
                    self.next();
                    let name = parts.join(".");
                    if name.eq_ignore_ascii_case("if") {
                        let cond = self.parse_or()?;
                        self.eat(&Tok::Comma)?;
                        let then = self.parse_or()?;
                        self.eat(&Tok::Comma)?;
                        let els = self.parse_or()?;
                        self.eat(&Tok::RParen)?;
                        return Ok(Node::If {
                            cond: Box::new(cond),
                            then: Box::new(then),
                            els: Box::new(els),
                        });
                    }
                    let agg_like = ["sum", "count", "avg", "min", "max"];
                    // 先解析第一个参数
                    let first = self.parse_or()?;
                    if let Tok::Ident(kw) = self.peek()
                        && kw.eq_ignore_ascii_case("where")
                    {
                        // SUM(expr WHERE cond)
                        self.next();
                        let cond = self.parse_or()?;
                        self.eat(&Tok::RParen)?;
                        return Ok(Node::Agg {
                            op: name.to_lowercase(),
                            expr: Box::new(first),
                            cond: Some(Box::new(cond)),
                        });
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
                            cond: None,
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

pub fn eval(node: &Node, lookup: Lookup) -> Result<Value, String> {
    match node {
        Node::Num(v) => Ok(Value::Num(*v)),
        Node::Str(s) => Ok(Value::Str(s.clone())),
        Node::Bool(b) => Ok(Value::Bool(*b)),
        Node::Path(p) => Ok(lookup(p).unwrap_or(Value::Empty)),
        Node::Not(x) => Ok(Value::Bool(!as_bool(&eval(x, lookup)?)?)),
        Node::Coalesce { l, r } => {
            let lv = eval(l, lookup)?;
            if lv != Value::Empty {
                Ok(lv)
            } else {
                eval(r, lookup)
            }
        }
        Node::If { cond, then, els } => {
            if as_bool(&eval(cond, lookup)?)? {
                eval(then, lookup)
            } else {
                eval(els, lookup)
            }
        }
        Node::Bin { op, l, r } => eval_bin(*op, &eval(l, lookup)?, &eval(r, lookup)?),
        Node::Call { name, args } => eval_call(name, args, lookup),
        Node::Agg { op, .. } => Err(format!("聚合 {op} 的行作用域求值尚未实现（M1 1.1）")),
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

fn eval_call(name: &str, args: &[Node], lookup: Lookup) -> Result<Value, String> {
    let lc = name.to_lowercase();
    match lc.as_str() {
        "min" | "max" => {
            if args.is_empty() {
                return Err("MIN/MAX 需要至少一个参数".into());
            }
            let mut nums = Vec::new();
            for a in args {
                nums.push(to_num(&eval(a, lookup)?)?);
            }
            let v = if lc == "min" {
                nums.into_iter().fold(f64::INFINITY, f64::min)
            } else {
                nums.into_iter().fold(f64::NEG_INFINITY, f64::max)
            };
            Ok(Value::Num(v))
        }
        "round" => {
            let v = to_num(&eval(&args[0], lookup)?)?;
            let digits = if args.len() > 1 {
                to_num(&eval(&args[1], lookup)?)? as i32
            } else {
                0
            };
            let f = 10f64.powi(digits);
            Ok(Value::Num((v * f).round() / f))
        }
        "abs" => Ok(Value::Num(to_num(&eval(&args[0], lookup)?)?.abs())),
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
    fn if_lazy() {
        assert_eq!(ev("IF(true, 1, 2)"), Value::Num(1.0));
        assert_eq!(ev("IF(false, 1, 2)"), Value::Num(2.0));
    }

    #[test]
    fn functions() {
        assert_eq!(ev("MIN(3, 1, 2)"), Value::Num(1.0));
        assert_eq!(ev("MAX(3, 1, 2)"), Value::Num(3.0));
        assert_eq!(ev("ROUND(1.26, 1)"), Value::Num(1.3));
        assert_eq!(ev("ABS(-4)"), Value::Num(4.0));
    }

    #[test]
    fn coalesce_and_missing_path() {
        assert_eq!(ev("课时.课时 ?? 0"), Value::Num(0.0));
        assert_eq!(ev("IF(true, 1, 2) ?? 9"), Value::Num(1.0));
    }

    #[test]
    fn chinese_identifiers_and_params() {
        assert_eq!(ev("IF(请假超额, 0, 课时.课时 ?? 0)"), Value::Num(0.0));
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
        // 行作用域求值待 M1-1.1；这里只验证语法可解析。
        parse("SUM(天数 WHERE 类型 != '公')").expect("聚合表达式应可解析");
        parse("MAX(0, 课时S - 参数.基础课时)").expect("普通调用应可解析");
    }

    #[test]
    fn parse_errors() {
        assert!(parse("1 +").is_err());
        assert!(parse("(1 + 2").is_err());
        assert!(parse("'abc").is_err());
    }
}
