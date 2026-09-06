//! 表达式代码编辑器（CodeEdit）与“按上下文动态补全”。
//!
//! 约定：
//! - 所有表达式/where 输入都用 [`CodeEdit`]（配置见 [`configure_code_edit`]），补全依赖
//!   Godot CodeEdit 的 `code_completion_requested` 信号；
//! - CodeEdit 不会在普通打字时自动请求补全（Godot 编辑器是收到 `text_changed` 后去请求的），
//!   因此这里把 `text_changed` → 判定是否“正在往表达式里敲词” → `request_code_completion()`；
//!   `code_completion_requested`（含 Ctrl+Space、`.` 前缀触发）→ 依据输入位置现算候选并弹出。
//! - 候选与引擎表达式语义一致：
//!   - 光标落在 `标识.` 之后（成员引用）：只给该命名空间的后缀名（如 `参数.` → 参数名，
//!     `qingjia.` → 该表列名），选中只插后缀；
//!   - 其它位置（整词引用）：给全名候选（`qingjia.天数`、`参数.考勤奖`），并给出全部语法关键字：
//!     函数/聚合（`SUM(`、`IF(`…）、逻辑（`AND/OR/NOT`）、空值/布尔（`空`/`true`…）；
//!     引擎对这些关键字大小写不敏感，候选会跟随用户当前输入的大小写给出对应形式；
//!     CodeEdit 会自动按光标前的词过滤并做整词替换。

use bevy::ecs::observer::On;
use bevy::ecs::system::NonSend;
use bevy::prelude::{Res, ResMut, Resource};
use godot::builtin::{Array, Color, Vector2};
use godot::classes::code_edit::CodeCompletionKind;
use godot::classes::control::SizeFlags;
use godot::classes::{CodeEdit, CodeHighlighter, SyntaxHighlighter};
use godot::obj::NewGd;
use godot::prelude::{GString, Gd};
use godot_bevy::interop::GodotNodeHandle;
use godot_bevy::prelude::GodotAccess;
use serde_json::Value;
use tracing::info;

use super::derived::DerivedDetail;
use super::editor::EditorDoc;
use super::events::{ExprCompletionRequested, ExprFocusEntered, ExprFocusExited, ExprTextChanged};

/// 表达式输入框的紧凑高度（一行）。
const COMPACT_HEIGHT: f32 = 42.0;
/// 获得焦点后的展开高度：给 CodeEdit 自绘的补全候选列表留出完整显示区域。
const EXPANDED_HEIGHT: f32 = 150.0;

// ---------- 语法高亮配色（CodeHighlighter，随方案命名空间动态刷新） ----------

/// 逻辑/空值/布尔关键字（AND/OR/NOT/空/true…）。
const C_KEYWORD: Color = Color::from_rgb(0.95, 0.55, 0.45);
/// 命名空间前缀 `参数`/`聚合`。
const C_NS_PARAM: Color = Color::from_rgb(0.95, 0.78, 0.4);
/// 表域 id（数据源/派生视图名，如 `qingjia`）。
const C_NS_TABLE: Color = Color::from_rgb(0.5, 0.8, 1.0);
/// 函数名（`SUM(`…；CodeHighlighter 按“词后跟 `(`”识别，大小写均可）。
const C_FUNC: Color = Color::from_rgb(0.5, 0.9, 0.95);
/// 属性成员（`.` 后的列名/参数名）。
const C_MEMBER: Color = Color::from_rgb(0.55, 0.92, 0.7);
/// 字符串字面量（`'...'` / `"..."`）。
const C_STRING: Color = Color::from_rgb(0.9, 0.68, 0.4);
/// 数字。
const C_NUMBER: Color = Color::from_rgb(0.88, 0.8, 0.55);
/// 符号/运算符。
const C_SYMBOL: Color = Color::from_rgb(0.58, 0.58, 0.6);

/// KPI_SHOT 自检下输出补全相关日志（日常使用不打印）。
fn dev_log(msg: String) {
    if std::env::var("KPI_SHOT").ok().as_deref() == Some("1") {
        info!("补全自检: {msg}");
    }
}

/// 候选种类：列/参数引用 ｜ 函数（`SUM(`…）｜ 语法关键字（`AND`/`空`/`true`…）。
#[derive(Clone, Copy, PartialEq, Eq)]
enum OptKind {
    Ref,
    Func,
    Kw,
}

/// 函数/聚合关键字。引擎对函数名大小写不敏感（解析时一律小写比较），
/// 因此补全会跟随用户当前输入的大小写给出对应形式（如 `SUM(` / `sum(`）。
const FUNC_NAMES: &[&str] = &[
    "IF", "MIN", "MAX", "ROUND", "ABS", "SUM", "AVG", "COUNT", "GROUP_BY",
];

/// 逻辑 / 空值 / 布尔关键字：AND/OR/NOT、true/false/null/empty 均大小写不敏感；`空` 固定中文。
const KEYWORD_NAMES: &[&str] = &["AND", "OR", "NOT", "空", "null", "empty", "true", "false"];

/// 依据光标前已输入词的形态决定候选大小写：已输入词全为小写字母 → 给全小写；否则给规范大写。
fn prefer_lower(before: &str) -> bool {
    let word = last_word(before);
    word.chars().any(|c| c.is_ascii_alphabetic()) && !word.chars().any(|c| c.is_ascii_uppercase())
}

/// 把关键字名按需要的形态输出（仅调整 ASCII 字母大小写，中文不受影响）。
fn styled(token: &str, lower: bool) -> String {
    if !lower {
        return token.to_string();
    }
    token
        .chars()
        .map(|c| {
            if c.is_ascii_uppercase() {
                c.to_ascii_lowercase()
            } else {
                c
            }
        })
        .collect()
}

/// 表达式输入框所在位置（决定语法上下文/候选集合）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditRole {
    /// 派生视图的行过滤 where。
    DerivedWhere,
    /// 派生视图列 expr。
    DerivedColExpr,
}

/// 文本长度快照：用于区分“打字”与“删除/程序 set_text”（删除时不自动弹补全）。
#[derive(Resource, Default)]
pub(crate) struct TextLenMap {
    /// (节点, 上次文本的字符数)。
    inner: Vec<(GodotNodeHandle, usize)>,
}

impl TextLenMap {
    fn get(&self, node: GodotNodeHandle) -> Option<usize> {
        self.inner
            .iter()
            .find(|(n, _)| *n == node)
            .map(|(_, len)| *len)
    }

    fn set(&mut self, node: GodotNodeHandle, len: usize) {
        if let Some(e) = self.inner.iter_mut().find(|(n, _)| *n == node) {
            e.1 = len;
        } else {
            // 简单的上界：只保留最近 64 个输入框的快照（旧的渲染框已释放，留着无意义）。
            if self.inner.len() >= 64 {
                self.inner.drain(0..32);
            }
            self.inner.push((node, len));
        }
    }
}

/// 统一配置一个表达式 CodeEdit：开补全、`.` 前缀触发、占位与最小尺寸，并挂上语法高亮。
/// where/expr 输入框后续新建时都应经由此入口，保证行为一致。
pub(crate) fn configure_code_edit(
    edit: &mut Gd<CodeEdit>,
    placeholder: &str,
    min_width: f32,
    expand: bool,
    raw: Option<&Value>,
) {
    edit.set_code_completion_enabled(true);
    let mut prefixes = Array::<GString>::new();
    let dot = GString::from(".");
    prefixes.push(&dot);
    edit.set_code_completion_prefixes(&prefixes);
    edit.set_placeholder(placeholder);
    edit.set_custom_minimum_size(Vector2::new(min_width, COMPACT_HEIGHT));
    if expand {
        edit.set_h_size_flags(SizeFlags::EXPAND_FILL);
    }
    apply_highlight(edit, raw);
}

/// 为表达式 CodeEdit 挂 CodeHighlighter：关键字/函数/命名空间/成员/字符串/数字分色。
/// CodeHighlighter 按“词是否紧跟 `(`”着函数色、按“`.` 后成员”着成员色、
/// 关键字/表 id/命名空间按词表精确匹配（大小写各自注册，引擎本就大小写不敏感）。
fn apply_highlight(edit: &mut Gd<CodeEdit>, raw: Option<&Value>) {
    let mut hl = CodeHighlighter::new_gd();
    hl.set_function_color(C_FUNC);
    hl.set_member_variable_color(C_MEMBER);
    hl.set_number_color(C_NUMBER);
    hl.set_symbol_color(C_SYMBOL);
    // 字符串字面量（DSL 用单/双引号）。
    hl.add_color_region("'", "'", C_STRING);
    hl.add_color_region("\"", "\"", C_STRING);

    // 逻辑/空值/布尔关键字（注册大小写两种，混合大小写极少数不在此高亮）。
    for &kw in KEYWORD_NAMES {
        hl.add_keyword_color(kw, C_KEYWORD);
        if kw.chars().any(|c| c.is_ascii_uppercase()) {
            let lower = kw.to_lowercase();
            hl.add_keyword_color(&lower, C_KEYWORD);
        }
        if kw.chars().any(|c| c.is_ascii_lowercase()) {
            let upper = kw.to_uppercase();
            hl.add_keyword_color(&upper, C_KEYWORD);
        }
    }
    // 命名空间前缀。
    hl.add_keyword_color("参数", C_NS_PARAM);
    hl.add_keyword_color("聚合", C_NS_PARAM);
    // 表域 id（数据源 + 派生），注册原样/大小写，方便按用户输入着色。
    if let Some(scheme) = raw.and_then(|r| r.get("scheme")) {
        for id in table_ids(scheme) {
            hl.add_keyword_color(&id, C_NS_TABLE);
            if id.chars().any(|c| c.is_ascii_uppercase()) {
                let lower = id.to_lowercase();
                hl.add_keyword_color(&lower, C_NS_TABLE);
            }
            if id.chars().any(|c| c.is_ascii_lowercase()) {
                let upper = id.to_uppercase();
                hl.add_keyword_color(&upper, C_NS_TABLE);
            }
        }
    }

    edit.set_syntax_highlighter(&hl.upcast::<SyntaxHighlighter>());
}

/// 方案中全部表域 id（数据源 + 派生，按声明顺序）。
fn table_ids(scheme: &Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(srcs) = scheme.get("sources").and_then(Value::as_array) {
        out.extend(
            srcs.iter()
                .filter_map(|s| s.get("id").and_then(Value::as_str).map(String::from)),
        );
    }
    if let Some(ds) = scheme.get("derived").and_then(Value::as_array) {
        out.extend(
            ds.iter()
                .filter_map(|d| d.get("id").and_then(Value::as_str).map(String::from)),
        );
    }
    out
}

// ---------- 聚焦展开/收起 ----------

/// 表达式 CodeEdit 聚焦：撑高到补全候选可完整显示的展开高度。
/// Godot 的补全列表是 CodeEdit 自绘内容，超出控件高度的部分会被后面的兄弟控件盖住，
/// 所以要在“弹补全时”让输入框自身有足够高的矩形。
pub(crate) fn on_expr_focus_entered(
    trigger: On<ExprFocusEntered>,
    detail: NonSend<DerivedDetail>,
    mut godot: GodotAccess,
) {
    let node = trigger.event().node;
    if role_of(&detail, node).is_none() {
        return;
    }
    let Some(mut edit) = godot.try_get::<CodeEdit>(node) else {
        return;
    };
    set_min_height(&mut edit, EXPANDED_HEIGHT);
    dev_log("focus_entered → 已展开编辑框高度".into());
}

/// 表达式 CodeEdit 失焦：恢复紧凑高度。
pub(crate) fn on_expr_focus_exited(
    trigger: On<ExprFocusExited>,
    detail: NonSend<DerivedDetail>,
    mut godot: GodotAccess,
) {
    let node = trigger.event().node;
    if role_of(&detail, node).is_none() {
        return;
    }
    let Some(mut edit) = godot.try_get::<CodeEdit>(node) else {
        return;
    };
    set_min_height(&mut edit, COMPACT_HEIGHT);
    dev_log("focus_exited → 已收起编辑框高度".into());
}

/// 只改最小高度，保留当前最小宽度。
fn set_min_height(edit: &mut Gd<CodeEdit>, height: f32) {
    let width = edit.get_custom_minimum_size().x;
    edit.set_custom_minimum_size(Vector2::new(width, height));
}

/// 判断输入框属于哪种表达式位置（通过实例 id 与详情控件对拍）。
fn role_of(detail: &DerivedDetail, node: GodotNodeHandle) -> Option<EditRole> {
    if detail
        .where_edit
        .as_ref()
        .is_some_and(|w| GodotNodeHandle::new(w.clone()) == node)
    {
        return Some(EditRole::DerivedWhere);
    }
    if detail
        .field_expr
        .iter()
        .any(|e| GodotNodeHandle::new(e.clone()) == node)
    {
        return Some(EditRole::DerivedColExpr);
    }
    None
}

// ---------- text_changed → 请求补全 ----------

/// CodeEdit `text_changed`：正在往表达式里敲词时请求补全。
pub(crate) fn on_expr_text_changed(
    trigger: On<ExprTextChanged>,
    detail: NonSend<DerivedDetail>,
    mut lens: ResMut<TextLenMap>,
    mut godot: GodotAccess,
) {
    let node = trigger.event().node;
    if role_of(&detail, node).is_none() {
        return;
    }
    let Some(mut edit) = godot.try_get::<CodeEdit>(node) else {
        return;
    };
    let len = edit.get_text().to_string().chars().count();
    // 失去焦点（程序 set_text / 载入重绘）不弹；仍同步长度供删除判定。
    if !edit.has_focus() {
        lens.set(node, len);
        return;
    }
    // 确认函数补全（插入 `xxx()`）后把光标移到括号中间，方便直接输参数。
    if place_caret_inside_parens(&mut edit) {
        lens.set(node, len);
        dev_log("函数补全 → 光标已移到括号中间".into());
        return;
    }
    // 删除类编辑（文本变短）不自动弹补全（可按 Ctrl+Space 手动触发）。
    if lens.get(node).is_some_and(|prev| len < prev) {
        lens.set(node, len);
        return;
    }
    lens.set(node, len);
    // 在字符串字面量内（如 `'公'`）不自动弹补全。
    let caret_line = edit.get_caret_line();
    let caret_col = edit.get_caret_column();
    if edit.is_in_string_ex(caret_line).column(caret_col).done() != -1 {
        return;
    }
    // 非强制请求：CodeEdit 按“光标前一字符是否是词字符/前缀”决定是否真的发起补全。
    edit.request_code_completion();
    dev_log("text_changed → 已调用 request_code_completion".into());
}

// ---------- code_completion_requested → 填候选 ----------

/// CodeEdit `code_completion_requested`：按输入框语义上下文现算候选并弹出。
pub(crate) fn on_expr_completion(
    trigger: On<ExprCompletionRequested>,
    detail: NonSend<DerivedDetail>,
    doc: Res<EditorDoc>,
    mut godot: GodotAccess,
) {
    let node = trigger.event().node;
    let Some(role) = role_of(&detail, node) else {
        return;
    };
    let Some(mut edit) = godot.try_get::<CodeEdit>(node) else {
        return;
    };
    let Some(raw) = doc.raw.as_ref() else {
        return;
    };
    let sel = doc.selected_derived.max(0) as usize;

    let before = caret_before(&edit);
    let opts = completion_options(role, sel, &detail, raw, &before);
    let preview: Vec<&str> = opts.iter().take(30).map(|o| o.text.as_str()).collect();
    dev_log(format!(
        "code_completion_requested role={role:?} before={before:?} 候选数={} {preview:?}",
        opts.len()
    ));
    let mut added = 0usize;
    for opt in opts {
        let kind = match opt.kind {
            OptKind::Func => CodeCompletionKind::FUNCTION,
            OptKind::Kw => CodeCompletionKind::PLAIN_TEXT,
            OptKind::Ref => CodeCompletionKind::VARIABLE,
        };
        edit.add_code_completion_option(kind, &opt.text, &opt.text);
        added += 1;
    }
    if added > 0 {
        edit.update_code_completion_options(false);
    }
}

/// 单个补全候选。
struct CompOpt {
    text: String,
    kind: OptKind,
}

fn comp(t: String) -> CompOpt {
    CompOpt {
        text: t,
        kind: OptKind::Ref,
    }
}

fn push_uniq(out: &mut Vec<CompOpt>, text: String, kind: OptKind) {
    if !out.iter().any(|o| o.text == text) {
        out.push(CompOpt { text, kind });
    }
}

/// 读取光标前（当前行、不含光标处）文本，用于判断“成员引用还是整词”。
fn caret_before(edit: &CodeEdit) -> String {
    let line = edit.get_caret_line();
    let col = edit.get_caret_column();
    let text = edit.get_line(line).to_string();
    let chars: Vec<char> = text.chars().collect();
    let col = (col.max(0) as usize).min(chars.len());
    chars[..col].iter().collect()
}

/// 若文本变化恰好是“刚确认函数补全（`名字()`，光标在 `)` 之后）”，
/// 则把光标移到一对括号中间；否则返回 false。
fn place_caret_inside_parens(edit: &mut Gd<CodeEdit>) -> bool {
    let line = edit.get_caret_line();
    let col = edit.get_caret_column();
    let text = edit.get_line(line).to_string();
    let chars: Vec<char> = text.chars().collect();
    let col = (col.max(0) as usize).min(chars.len());
    // 需要形如 `名字()` 且光标在 `)` 之后。
    if col < 2 || chars[col - 2] != '(' || chars[col - 1] != ')' {
        return false;
    }
    // `(` 前应是一个词的末尾（函数名），避免对“路径()”之类误判。
    if col < 3 || !is_word_char(chars[col - 3]) {
        return false;
    }
    edit.set_caret_column((col - 1) as i32);
    true
}

/// 仿 Godot 的 is_symbol 词元分隔集（`.`、空格、运算符等都算分隔；中文/`、`等算词字符）。
fn is_sep(c: char) -> bool {
    matches!(
        c,
        ' ' | '\t'
            | '.'
            | ','
            | ';'
            | ':'
            | '/'
            | '\\'
            | '~'
            | '`'
            | '!'
            | '%'
            | '^'
            | '&'
            | '*'
            | '('
            | ')'
            | '='
            | '+'
            | '['
            | ']'
            | '{'
            | '}'
            | '<'
            | '>'
            | '|'
            | '#'
            | '@'
            | '"'
            | '\''
            | '-'
    )
}

fn is_word_char(c: char) -> bool {
    !is_sep(c)
}

/// 从光标前文本里找最后一个 `.` 前最近的“词”（成员属主）。
fn member_owner(before: &str) -> Option<String> {
    let dot = before.rfind('.')?;
    let owner = last_word(&before[..dot]);
    let suffix = &before[dot + 1..];
    if owner.is_empty() || !suffix.chars().all(is_word_char) {
        return None;
    }
    Some(owner)
}

/// 取字符串末尾的最后一个词（以分隔符为界）。
fn last_word(s: &str) -> String {
    s.chars()
        .rev()
        .take_while(|c| is_word_char(*c))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

// ---------- 候选生成（依据方案 JSON 现状，保证与引擎一致） ----------

/// 计算某个输入框此刻应弹出的候选。
fn completion_options(
    role: EditRole,
    sel: usize,
    detail: &DerivedDetail,
    raw: &Value,
    before: &str,
) -> Vec<CompOpt> {
    // 1) 成员引用：光标紧跟 `属主.` 后（或正在输入其成员词），只给该属主下的后缀候选。
    if let Some(owner) = member_owner(before) {
        return member_pool(detail, raw, sel, &owner).unwrap_or_default();
    }
    // 2) 整词引用：给完整候选（含函数/关键字），由 CodeEdit 按光标前的词过滤。
    full_pool(role, detail, raw, sel, before)
}

/// 详情 from 下拉当前选择；为空回退 raw。
fn live_from(detail: &DerivedDetail, raw: &Value, sel: usize) -> Option<String> {
    if let Some(from) = detail.from_btn.as_ref() {
        let t = from.get_text().to_string().trim().to_string();
        if !t.is_empty() {
            return Some(t);
        }
    }
    derived_at(raw, sel)?
        .get("from")?
        .as_str()
        .map(String::from)
}

/// 详情面板里当前派生可见的列名（视图列名编辑框里的实时值 + raw 已存值，去重保序）。
fn live_own_cols(detail: &DerivedDetail, raw: &Value, sel: usize) -> Vec<String> {
    let mut out = Vec::new();
    for e in &detail.field_name {
        let t = e.get_text().to_string().trim().to_string();
        if !t.is_empty() && !out.contains(&t) {
            out.push(t);
        }
    }
    if let Some(d) = derived_at(raw, sel)
        && let Some(cols) = d.get("columns").and_then(Value::as_array)
    {
        for c in cols {
            if let Some(name) = c.get("name").and_then(Value::as_str) {
                let name = name.trim();
                if !name.is_empty() && !out.iter().any(|o| o.as_str() == name) {
                    out.push(name.to_string());
                }
            }
        }
    }
    out
}

fn derived_at(raw: &Value, sel: usize) -> Option<&Value> {
    raw.get("scheme")?.get("derived")?.as_array()?.get(sel)
}

/// 某“表”（源或派生）可引用的列名。派生若未显式定义列（透传视图），则沿用其 from 表列。
fn table_columns(raw: &Value, id: &str) -> Option<Vec<String>> {
    table_columns_depth(raw, id, 0)
}

fn table_columns_depth(raw: &Value, id: &str, depth: usize) -> Option<Vec<String>> {
    if depth > 8 {
        return None; // 防环（透传派生链异常时）
    }
    let scheme = raw.get("scheme")?;
    // 数据源
    if let Some(cols) = source_columns(scheme.get("sources")?, id) {
        return Some(cols);
    }
    // 派生视图
    let derived = scheme.get("derived")?.as_array()?;
    let d = derived
        .iter()
        .find(|s| s.get("id").and_then(Value::as_str) == Some(id))?;
    if let Some(cols) = derived_columns(d) {
        return Some(cols);
    }
    // 透传派生（无显式列）：列集 = from 表列
    let from = d.get("from").and_then(Value::as_str)?;
    table_columns_depth(raw, from, depth + 1)
}

/// 数据源列：引用用 alias（缺省退回 header）。找到表即返回（可能为空）。
fn source_columns(arr: &Value, id: &str) -> Option<Vec<String>> {
    let list = arr.as_array()?;
    let item = list
        .iter()
        .find(|s| s.get("id").and_then(Value::as_str) == Some(id))?;
    let cols = item.get("columns").and_then(Value::as_array)?;
    let mut out = Vec::new();
    for c in cols {
        let name = c
            .get("alias")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.trim())
            .or_else(|| c.get("header").and_then(Value::as_str).map(str::trim));
        if let Some(name) = name
            && !name.is_empty()
            && !out.iter().any(|o: &String| o.as_str() == name)
        {
            out.push(name.to_string());
        }
    }
    Some(out)
}

/// 派生显式列名；未声明（透传）返回 None。
fn derived_columns(d: &Value) -> Option<Vec<String>> {
    let cols = d.get("columns").and_then(Value::as_array)?;
    if cols.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    for c in cols {
        if let Some(name) = c.get("name").and_then(Value::as_str) {
            let name = name.trim();
            if !name.is_empty() && !out.iter().any(|o: &String| o.as_str() == name) {
                out.push(name.to_string());
            }
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

fn param_names(raw: &Value) -> Vec<String> {
    raw.get("scheme")
        .and_then(|s| s.get("parameters"))
        .and_then(Value::as_array)
        .map(|ps| {
            ps.iter()
                .filter_map(|p| p.get("name").and_then(Value::as_str).map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// 成员池：`属主.` 后的可选项。属主必须是已识别的命名空间（参数/聚合/表 id）。
fn member_pool(
    detail: &DerivedDetail,
    raw: &Value,
    sel: usize,
    owner: &str,
) -> Option<Vec<CompOpt>> {
    // `参数.` → 参数名
    if owner == "参数" {
        return Some(param_names(raw).into_iter().map(comp).collect());
    }
    // `聚合.` → 已声明聚合 id（为后续聚合页预留）
    if owner == "聚合" {
        let aggs: Vec<String> = raw
            .get("scheme")
            .and_then(|s| s.get("aggregates"))
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.get("id").and_then(Value::as_str).map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        return Some(aggs.into_iter().map(comp).collect());
    }
    // 当前派生自身（其视图列，含面板上刚输入的列名）：
    if let Some((own_id, cols)) = own_view(detail, raw, sel)
        && owner == own_id
    {
        return Some(cols.into_iter().map(comp).collect());
    }
    // 其它表（源/派生）列
    if let Some(cols) = table_columns(raw, owner) {
        return Some(cols.into_iter().map(comp).collect());
    }
    // 未识别属主（打错字 / 数字字面量如 `1.`）：不弹，避免拼出脏引用。
    None
}

/// 当前派生视图的 (id, 可见列名)。
fn own_view(detail: &DerivedDetail, raw: &Value, sel: usize) -> Option<(String, Vec<String>)> {
    let d = derived_at(raw, sel)?;
    let id = detail
        .id_edit
        .as_ref()
        .map(|e| e.get_text().to_string())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            d.get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        });
    if id.is_empty() {
        return None;
    }
    Some((id, live_own_cols(detail, raw, sel)))
}

/// 整词候选（完整引用 + 关键字 + 函数），供 CodeEdit 按当前词过滤/整词替换。
/// 函数/关键字的大小写跟随用户当前输入（引擎对它们大小写不敏感）。
fn full_pool(
    role: EditRole,
    detail: &DerivedDetail,
    raw: &Value,
    sel: usize,
    before: &str,
) -> Vec<CompOpt> {
    let mut out: Vec<CompOpt> = Vec::new();
    let from = live_from(detail, raw, sel);
    let lower = prefer_lower(before);

    if let Some(from) = &from
        && let Some(cols) = table_columns(raw, from)
    {
        for c in cols {
            push_uniq(&mut out, format!("{from}.{c}"), OptKind::Ref);
        }
    }

    // 当前派生自身前缀（仅列 expr；组内“本视图前列”引用）
    if role == EditRole::DerivedColExpr
        && let Some((own_id, cols)) = own_view(detail, raw, sel)
    {
        for c in cols {
            push_uniq(&mut out, format!("{own_id}.{c}"), OptKind::Ref);
        }
    }

    for p in param_names(raw) {
        push_uniq(&mut out, format!("参数.{p}"), OptKind::Ref);
    }

    // 语法关键字：AND/OR/NOT、空值、布尔（where 与列 expr 都可用）。
    for kw in KEYWORD_NAMES {
        push_uniq(&mut out, styled(kw, lower), OptKind::Kw);
    }
    // 函数/聚合关键字：仅列 expr（行集聚合 / 组键声明在此语境才有意义）。
    // 带上一对括号，用户确认后由 place_caret_inside_parens 把光标移到括号中间。
    if role == EditRole::DerivedColExpr {
        for f in FUNC_NAMES {
            push_uniq(&mut out, format!("{}()", styled(f, lower)), OptKind::Func);
        }
    }
    out
}
