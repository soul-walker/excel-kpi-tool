//! exec：方案执行器内核（M1-1.3 起点）。
//!
//! 数据流：sources（物理表，文件唯一）→ derived（表视图，可链式）→
//! aggregates（按 key 聚合）/ groups（字段求值）/ outputs。
//! derived 视图变换顺序：主从合并(merge) → 行过滤(when) →
//! 视图列 columns 逐行求值 / 某列 `GROUP_BY(...)` 时按组汇总。

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use calamine::Reader as _;

use crate::calc::{self, Node, Row as CalcRow, Value};
use crate::error::{Error, Result};
use crate::reader::{cell_ref, normalize_key, read_source_table};
use crate::scheme::{DerivedSpec, OutputKind, Scheme};
use crate::table::{CellValue, Table};
use crate::template::SheetEdit;

/// 一次运行的中间产物。
pub struct SchemeRun {
    pub source_tables: HashMap<String, Table>,
    pub derived_tables: HashMap<String, DerivedTable>,
}

/// 派生结果：列顺序 + 行（行 = 列 alias → 值）。
#[derive(Debug, Clone)]
pub struct DerivedTable {
    pub columns: Vec<String>,
    pub rows: Vec<CalcRow>,
}

/// 把相对文件名绑定到月份目录：`file` 直接是相对月份目录的文件名；
/// 兼容旧版 `{monthDir}/xxx` 前缀写法（剥掉前缀，兜底把其它位置的 `{monthDir}` 换成 "."）。
pub fn resolve_file(pattern: &str, dir: &Path) -> PathBuf {
    // 前缀形式 `{monthDir}/xxx` 直接剥掉；兜底把其它位置的 `{monthDir}` 换成 "."。
    let rel = pattern
        .replacen("{monthDir}/", "", 1)
        .replace("{monthDir}", ".");
    dir.join(rel)
}

/// 运行一个方案的 sources + derived 阶段。
pub fn run_scheme(scheme: &Scheme, dir: &Path) -> Result<SchemeRun> {
    let mut source_tables = HashMap::new();
    for s in &scheme.sources {
        let path = resolve_file(&s.file, dir);
        let table = read_source_table(&path, s)?;
        source_tables.insert(s.id.clone(), table);
    }

    let mut derived_tables = HashMap::new();
    let params = param_lookup(scheme);
    for d in &scheme.derived {
        let cols = cols_for(&d.from, &source_tables, &derived_tables)?;
        let rows = rows_for(&d.from, &source_tables, &derived_tables)?;
        let table = compute_derived(d, &params, &cols, &rows)?;
        derived_tables.insert(d.id.clone(), table);
    }

    Ok(SchemeRun {
        source_tables,
        derived_tables,
    })
}

/// 来源表的列 alias（顺序）。
fn cols_for(
    id: &str,
    sources: &HashMap<String, Table>,
    derived: &HashMap<String, DerivedTable>,
) -> Result<Vec<String>> {
    if let Some(t) = sources.get(id) {
        return Ok(t.columns.iter().map(|c| c.alias.clone()).collect());
    }
    if let Some(d) = derived.get(id) {
        return Ok(d.columns.clone());
    }
    Err(Error::Runtime(format!("引用不存在的表: {id}")))
}

fn rows_for(
    id: &str,
    sources: &HashMap<String, Table>,
    derived: &HashMap<String, DerivedTable>,
) -> Result<Vec<CalcRow>> {
    if let Some(t) = sources.get(id) {
        return Ok(table_to_rows(t));
    }
    if let Some(d) = derived.get(id) {
        return Ok(d.rows.clone());
    }
    Err(Error::Runtime(format!("引用不存在的表: {id}")))
}

/// 规整 Table → calc 行（列 alias → 值）。
pub fn table_to_rows(t: &Table) -> Vec<CalcRow> {
    let aliases: Vec<&str> = t.columns.iter().map(|c| c.alias.as_str()).collect();
    t.rows
        .iter()
        .map(|r| {
            let mut map = CalcRow::new();
            for (i, a) in aliases.iter().enumerate() {
                map.insert(a.to_string(), cell_to_value(&r.cells[i]));
            }
            map
        })
        .collect()
}

pub fn cell_to_value(c: &CellValue) -> Value {
    match c {
        CellValue::Text(s) => Value::Str(s.clone()),
        CellValue::Integer(i) => Value::Num(*i as f64),
        CellValue::Float(f) => Value::Num(*f),
        CellValue::Boolean(b) => Value::Bool(*b),
        CellValue::Blank => Value::Empty,
    }
}

fn value_key(v: &Value) -> String {
    match v {
        Value::Str(s) => s.clone(),
        Value::Num(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Empty => String::new(),
    }
}

/// 执行派生（表视图）：主从合并 → 行过滤 → 按“视图列定义”逐行/分组产出。
/// 表达式引用一律带前缀：`{from}.列` 在解析后归并为行级列；`{本视图}.列` 引用前列；
/// `参数.x` 引用方案参数。某列写 `GROUP_BY(...)` 即声明该列为组键（汇总视图）。
pub fn compute_derived(
    spec: &DerivedSpec,
    params: &HashMap<String, Value>,
    in_cols: &[String],
    in_rows: &[CalcRow],
) -> Result<DerivedTable> {
    // 0) 主从合并：续列全空的续行并入上一条主记录（保留原列）。
    let base: Vec<CalcRow> = match &spec.merge {
        Some(m) => merge_continuations(in_rows, &m.on)?,
        None => in_rows.to_vec(),
    };

    // 1) 过滤（when 引用同样带 from 前缀，先归并）
    let mut when_node = spec
        .when
        .as_deref()
        .map(calc::parse)
        .transpose()
        .map_err(|e| Error::Runtime(format!("derived.{} when 解析失败: {e}", spec.id)))?;
    if let Some(n) = &mut when_node {
        strip_from_prefix(n, &spec.from);
    }
    let mut rows: Vec<CalcRow> = Vec::with_capacity(base.len());
    for r in base {
        let keep = match &when_node {
            Some(n) => {
                let v = calc::eval_on_row(n, &|_| None, &r).map_err(|e| {
                    Error::Runtime(format!("derived.{} when 求值失败: {e}", spec.id))
                })?;
                matches!(v, Value::Bool(true)) || matches!(v, Value::Num(x) if x != 0.0)
            }
            None => true,
        };
        if keep {
            rows.push(r);
        }
    }

    // 2) 视图列定义：解析表达式、归并 from 前缀，并识别 GROUP_BY 键列。
    if spec.columns.is_empty() {
        return Ok(DerivedTable {
            columns: in_cols.to_vec(),
            rows,
        });
    }
    let mut parsed: Vec<(String, Option<Node>)> = Vec::with_capacity(spec.columns.len());
    for c in &spec.columns {
        let mut node = c
            .expr
            .as_deref()
            .map(calc::parse)
            .transpose()
            .map_err(|e| {
                Error::Runtime(format!(
                    "derived.{} 列 {} 表达式解析失败: {e}",
                    spec.id, c.name
                ))
            })?;
        if let Some(n) = &mut node {
            strip_from_prefix(n, &spec.from);
        }
        parsed.push((c.name.clone(), node));
    }
    let columns: Vec<String> = parsed.iter().map(|(n, _)| n.clone()).collect();
    let key_col: Option<usize> = parsed.iter().position(|(_, n)| {
        matches!(n, Some(Node::Call { name, .. }) if name.eq_ignore_ascii_case("group_by"))
    });

    // 共享 resolver：`{本视图}.列` → 前列输出；`参数.x` → 参数；裸名 → 前列（宽容）。
    let resolver = |p: &[String], comp: &HashMap<String, Value>| -> Option<Value> {
        if p.len() == 1 {
            comp.get(&p[0]).cloned()
        } else if p.len() == 2 && p[0] == spec.id {
            comp.get(&p[1]).cloned()
        } else if p.len() == 2 && p[0] == "参数" {
            params.get(&p[1]).cloned()
        } else {
            None
        }
    };

    if let Some(kidx) = key_col {
        // 分组汇总视图：键列表达式对每行求值得到组键。
        let key_node = match &parsed[kidx].1 {
            Some(Node::Call { args, .. }) => args.first().cloned().ok_or_else(|| {
                Error::Runtime(format!(
                    "derived.{}：GROUP_BY 需要参数（如 GROUP_BY({}.列))",
                    spec.id, spec.from
                ))
            })?,
            _ => unreachable!(),
        };
        let mut groups: BTreeMap<String, Vec<CalcRow>> = BTreeMap::new();
        for r in &rows {
            let k = calc::eval_on_row(&key_node, &|_| None, r)
                .map_err(|e| Error::Runtime(format!("derived.{} 分组键求值失败: {e}", spec.id)))?;
            groups.entry(value_key(&k)).or_default().push(r.clone());
        }
        let mut out_rows = Vec::with_capacity(groups.len());
        for (_k, g) in groups {
            let mut comp: HashMap<String, Value> = HashMap::new();
            let mut out = CalcRow::new();
            for (idx, (name, node)) in parsed.iter().enumerate() {
                let val = if idx == kidx {
                    // 键列：取组内首行同键值（组内一致）
                    calc::eval_on_row(&key_node, &|_| None, &g[0]).map_err(|e| {
                        Error::Runtime(format!("derived.{} 列 {name} 求值失败: {e}", spec.id))
                    })?
                } else {
                    match node {
                        Some(n) => {
                            calc::eval_rows(n, &|p| resolver(p, &comp), &g).map_err(|e| {
                                Error::Runtime(format!(
                                    "derived.{} 列 {name} 求值失败: {e}",
                                    spec.id
                                ))
                            })?
                        }
                        None => g
                            .first()
                            .and_then(|f| f.get(name))
                            .cloned()
                            .unwrap_or(Value::Empty),
                    }
                };
                comp.insert(name.clone(), val.clone());
                out.insert(name.clone(), val);
            }
            out_rows.push(out);
        }
        return Ok(DerivedTable {
            columns,
            rows: out_rows,
        });
    }

    // 无分组：每行按列定义逐行求值（可引用前列与参数）。
    let mut out_rows = Vec::with_capacity(rows.len());
    for r in &rows {
        let mut comp: HashMap<String, Value> = HashMap::new();
        let mut out = CalcRow::new();
        for (name, node) in &parsed {
            let val = match node {
                Some(n) => calc::eval_on_row(n, &|p| resolver(p, &comp), r).map_err(|e| {
                    Error::Runtime(format!("derived.{} 列 {name} 求值失败: {e}", spec.id))
                })?,
                None => r.get(name).cloned().unwrap_or(Value::Empty),
            };
            comp.insert(name.clone(), val.clone());
            out.insert(name.clone(), val);
        }
        out_rows.push(out);
    }
    Ok(DerivedTable {
        columns,
        rows: out_rows,
    })
}

/// 把 AST 中的 `{from}.列` 前缀归并为裸列名（仅显式书写，执行语义 = 输入行列）。
fn strip_from_prefix(node: &mut Node, from: &str) {
    match node {
        Node::Path(p) => {
            if p.len() >= 2 && p[0] == from {
                *p = p[1..].to_vec();
            }
        }
        Node::Not(x) => strip_from_prefix(x, from),
        Node::Coalesce { l, r } => {
            strip_from_prefix(l, from);
            strip_from_prefix(r, from);
        }
        Node::If { cond, then, els } => {
            strip_from_prefix(cond, from);
            strip_from_prefix(then, from);
            strip_from_prefix(els, from);
        }
        Node::Bin { l, r, .. } => {
            strip_from_prefix(l, from);
            strip_from_prefix(r, from);
        }
        Node::Call { args, .. } => {
            for a in args {
                strip_from_prefix(a, from);
            }
        }
        Node::Agg { expr, cond, .. } => {
            strip_from_prefix(expr, from);
            if let Some(c) = cond {
                strip_from_prefix(c, from);
            }
        }
        _ => {}
    }
}

/// 主从续行合并：`on` 列全空的物理行并入上一条主记录。
/// 与读取期的源级 `continue_record_on` 语义一致：数值求和、文本以“、”追加去重，
/// 续列本身与空值保持主记录值。
fn merge_continuations(rows: &[CalcRow], on: &[String]) -> Result<Vec<CalcRow>> {
    if on.is_empty() {
        return Ok(rows.to_vec());
    }
    let mut merged: Vec<CalcRow> = Vec::with_capacity(rows.len());
    for row in rows {
        // 主记录：续列全非空；续列全空的行并入上一条主记录。
        let is_lead = on.iter().all(|k| !value_blank(row.get(k)));
        if is_lead || merged.is_empty() {
            merged.push(row.clone());
        } else if let Some(last) = merged.last_mut() {
            for (k, v) in row {
                if on.iter().any(|c| c == k) || value_blank(Some(v)) {
                    continue;
                }
                if let Some(target) = last.get_mut(k) {
                    merge_value_into(target, v);
                } else {
                    last.insert(k.clone(), v.clone());
                }
            }
        }
    }
    Ok(merged)
}

fn value_blank(v: Option<&Value>) -> bool {
    matches!(v, None | Some(Value::Empty))
}

fn merge_value_into(target: &mut Value, extra: &Value) {
    match (&*target, extra) {
        (Value::Empty, _) => *target = extra.clone(),
        (Value::Num(a), Value::Num(b)) => *target = Value::Num(a + b),
        (Value::Str(a), Value::Str(b)) if !a.contains(b.as_str()) => {
            let mut s = a.clone();
            if !s.is_empty() {
                s.push('、');
            }
            s.push_str(b);
            *target = Value::Str(s);
        }
        _ => {}
    }
}

// ---- 按人聚合 / 分组字段逐人求值（绩效复刻核心）----

use std::collections::BTreeSet;

/// 明细类 id：被任一 aggregate 引用的表（行粒度，不是“一人一行”）。
pub fn detail_ids(scheme: &Scheme) -> BTreeSet<String> {
    scheme.aggregates.iter().map(|a| a.from.clone()).collect()
}

fn person_col(scheme: &Scheme) -> String {
    scheme.entity.key_field.clone()
}

fn key_value<'a>(row: &'a CalcRow, key: &str) -> Option<&'a Value> {
    row.get(key)
}

/// 参数表：路径 `参数.x` 或裸名 x → 值。
pub fn param_lookup(scheme: &Scheme) -> HashMap<String, Value> {
    let mut m = HashMap::new();
    for p in &scheme.parameters {
        let v = match &p.value {
            crate::scheme::ParamValue::Integer(i) => Value::Num(*i as f64),
            crate::scheme::ParamValue::Float(f) => Value::Num(*f),
            crate::scheme::ParamValue::Boolean(b) => Value::Bool(*b),
            crate::scheme::ParamValue::Text(s) => Value::Str(s.clone()),
        };
        m.insert(p.name.clone(), v);
    }
    m
}

/// 聚合结果：agg.id → (person → value)。依赖按 scheme.aggregates 顺序（引用前序聚合）。
pub fn compute_aggregates(
    scheme: &Scheme,
    run: &SchemeRun,
) -> Result<BTreeMap<String, BTreeMap<String, Value>>> {
    let params = param_lookup(scheme);
    let mut out: BTreeMap<String, BTreeMap<String, Value>> = BTreeMap::new();

    for agg in &scheme.aggregates {
        let rows = rows_for(&agg.from, &run.source_tables, &run.derived_tables)?;
        let mut by_person: BTreeMap<String, Vec<CalcRow>> = BTreeMap::new();
        for r in rows {
            if let Some(v) = key_value(&r, &agg.group_by) {
                by_person
                    .entry(normalize_key(&value_key(v)))
                    .or_default()
                    .push(r);
            }
        }
        let node = calc::parse(&agg.expr)
            .map_err(|e| Error::Runtime(format!("聚合 {} 表达式解析失败: {e}", agg.id)))?;

        let mut per_person: BTreeMap<String, Value> = BTreeMap::new();
        for (person, group_rows) in &by_person {
            let val = calc::eval_rows(
                &node,
                &|p| resolve_plain(p, &out, &params, person),
                group_rows,
            )
            .map_err(|e| Error::Runtime(format!("聚合 {} 求值失败: {e}", agg.id)))?;
            per_person.insert(person.clone(), val);
        }
        out.insert(agg.id.clone(), per_person);
    }
    Ok(out)
}

/// 解析：单段路径 → 已算聚合(按人) 或 参数裸名；两段 `参数.x` → 参数。
fn resolve_plain(
    p: &[String],
    aggs: &BTreeMap<String, BTreeMap<String, Value>>,
    params: &HashMap<String, Value>,
    person: &str,
) -> Option<Value> {
    match p {
        [single] => {
            if let Some(by_person) = aggs.get(single)
                && let Some(v) = by_person.get(person)
            {
                return Some(v.clone());
            }
            params.get(single).cloned()
        }
        _ => {
            if p.len() == 2 && p[0] == "参数" {
                params.get(&p[1]).cloned()
            } else {
                None
            }
        }
    }
}

/// 分组字段有效集：以 groups[0] 为共享基础字段，后续组用 overrides 覆盖/停用。
pub fn effective_fields(scheme: &Scheme, group_id: &str) -> Result<Vec<(String, Option<String>)>> {
    let base = scheme
        .groups
        .first()
        .ok_or_else(|| Error::Runtime("方案缺少计算分组".into()))?;
    let mut map: Vec<(String, Option<String>)> = base
        .fields
        .iter()
        .map(|f| (f.name.clone(), f.expr.clone()))
        .collect();
    let group = scheme
        .groups
        .iter()
        .find(|g| g.id == group_id)
        .ok_or_else(|| Error::Runtime(format!("找不到分组 {group_id}")))?;
    for o in &group.overrides {
        if let Some(pos) = map.iter().position(|(n, _)| n == &o.name) {
            map[pos].1 = o.expr.clone();
        } else if o.expr.is_some() {
            map.push((o.name.clone(), o.expr.clone()));
        }
    }
    Ok(map)
}

/// 计算某人在某组的全部字段。
pub fn compute_group_fields_for(
    scheme: &Scheme,
    run: &SchemeRun,
    aggs: &BTreeMap<String, BTreeMap<String, Value>>,
    group_id: &str,
    person: &str,
) -> Result<BTreeMap<String, Value>> {
    let fields = effective_fields(scheme, group_id)?;
    let params = param_lookup(scheme);
    let key = person_col(scheme);
    let details = detail_ids(scheme);
    let mut computed: BTreeMap<String, Value> = BTreeMap::new();

    for (name, expr_text) in fields {
        let Some(text) = expr_text else {
            continue;
        };
        let node =
            calc::parse(&text).map_err(|e| Error::Runtime(format!("字段 {name} 解析失败: {e}")))?;
        let value = calc::eval(&node, &|p| {
            if let [single] = p {
                if let Some(v) = computed.get(single) {
                    return Some(v.clone());
                }
                if let Some(by_person) = aggs.get(single) {
                    let want = normalize_key(person);
                    if let Some(v) = by_person.get(&want).or_else(|| by_person.get(person)) {
                        return Some(v.clone());
                    }
                }
                return params.get(single).cloned();
            }
            if p.len() == 2 && p[0] == "参数" {
                return params.get(&p[1]).cloned();
            }
            if p.len() == 2 && !details.contains(&p[0]) {
                return lookup_scalar(run, &p[0], &key, person, &p[1]);
            }
            None
        })
        .map_err(|e| Error::Runtime(format!("字段 {name} 求值失败: {e}")))?;
        computed.insert(name, value);
    }
    Ok(computed)
}

fn lookup_scalar(
    run: &SchemeRun,
    src: &str,
    key_col: &str,
    person: &str,
    col: &str,
) -> Option<Value> {
    let rows = match (run.source_tables.get(src), run.derived_tables.get(src)) {
        (Some(t), _) => table_to_rows(t),
        (None, Some(d)) => d.rows.clone(),
        (None, None) => return None,
    };
    let want = normalize_key(person);
    rows.into_iter()
        .find(|r| {
            key_value(r, key_col)
                .map(|v| normalize_key(&value_key(v)))
                .as_deref()
                == Some(want.as_str())
        })
        .and_then(|r| r.get(col).cloned())
}

// ---- 模板回写规划（patch_template 输出 → SheetEdit）----

fn cell_as_text(d: &calamine::Data) -> Option<String> {
    use calamine::Data;
    match d {
        Data::String(s) => Some(s.clone()),
        Data::Int(i) => Some(i.to_string()),
        Data::Float(f) => Some(f.to_string()),
        Data::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// 规划某个 patch_template 输出的写入单元格。
///
/// - 按每个 (group → sheet) 用输出列表定位模板表头行与人员行；
/// - 每个输出列：字段值为 Num → 写数值；为 空 → 清空该单元格（旧代码写空字符串）；
/// - 模板中不存在的表头列（如 校聘 sheet 缺"上浮30%"）自动跳过。
#[allow(clippy::too_many_arguments)]
pub fn plan_template_patch(
    scheme: &Scheme,
    run: &SchemeRun,
    aggs: &BTreeMap<String, BTreeMap<String, Value>>,
    output_id: &str,
    month_dir: &Path,
) -> Result<Vec<SheetEdit>> {
    let out = scheme
        .outputs
        .iter()
        .find(|o| o.id == output_id)
        .ok_or_else(|| Error::Runtime(format!("找不到输出 {output_id}")))?;
    let p = match &out.kind {
        OutputKind::PatchTemplate(p) => p,
        _ => {
            return Err(Error::Runtime(format!(
                "输出 {output_id} 不是 patch_template"
            )));
        }
    };
    let path = resolve_file(&p.file, month_dir);
    if !path.exists() {
        return Err(Error::Runtime(format!("模板不存在: {path:?}")));
    }
    let key_header = p.row_key.as_deref().unwrap_or(&scheme.entity.key_field);

    let mut wb = calamine::open_workbook_auto(&path)
        .map_err(|e| Error::Runtime(format!("打开 {path:?} 失败: {e}")))?;
    let norm_cols: BTreeSet<String> = p.columns.iter().map(|c| normalize_key(&c.header)).collect();

    let mut edits = Vec::new();
    for g in &p.groups {
        if !wb.sheet_names().iter().any(|n| n == &g.sheet) {
            return Err(Error::Runtime(format!("sheet 不存在: {}", g.sheet)));
        }
        let range = wb
            .worksheet_range(&g.sheet)
            .map_err(|e| Error::Runtime(format!("读取 sheet {} 失败: {e}", g.sheet)))?;
        let (nrows, ncols) = range.get_size();

        // 找表头行：含 key 表头且命中 ≥3 个输出表头的最靠前行。
        let mut header_row: Option<u32> = None;
        for r in 0..nrows.min(20) {
            let has_key = (0..ncols).any(|c| {
                range
                    .get_value((r as u32, c as u32))
                    .and_then(cell_as_text)
                    .map(|t| normalize_key(&t) == normalize_key(key_header))
                    .unwrap_or(false)
            });
            if !has_key {
                continue;
            }
            let hits = (0..ncols)
                .filter_map(|c| range.get_value((r as u32, c as u32)).and_then(cell_as_text))
                .filter(|t| norm_cols.contains(&normalize_key(t)))
                .count();
            if hits >= 3 {
                header_row = Some(r as u32);
                break;
            }
        }
        let hr = header_row.ok_or_else(|| {
            Error::Runtime(format!("sheet {} 未找到含 {key_header} 的表头行", g.sheet))
        })?;

        // 表头名(归一化) → 列号(1 起始)。
        let mut col_of: HashMap<String, u32> = HashMap::new();
        for c in 0..ncols {
            if let Some(t) = range.get_value((hr, c as u32)).and_then(cell_as_text) {
                col_of.insert(normalize_key(&t), c as u32 + 1);
            }
        }
        let name_col = col_of
            .get(&normalize_key(key_header))
            .copied()
            .ok_or_else(|| {
                Error::Runtime(format!("sheet {} 缺 key 列表头 {key_header}", g.sheet))
            })?;

        let mut cells: Vec<(String, Option<String>)> = Vec::new();
        for r in (hr as usize + 1)..nrows {
            let Some(text) = range
                .get_value((r as u32, (name_col - 1) as u32))
                .and_then(cell_as_text)
            else {
                continue;
            };
            let person = normalize_key(&text);
            if person.is_empty() {
                continue;
            }
            let fields =
                compute_group_fields_for(scheme, run, aggs, &g.group, &person).map_err(|e| {
                    Error::Runtime(format!("计算 {person} @ {} 字段失败: {e}", g.sheet))
                })?;
            for col in &p.columns {
                let Some(cnum) = col_of.get(&normalize_key(&col.header)) else {
                    continue;
                };
                let write = match fields.get(&col.field) {
                    Some(Value::Num(n)) => Some(format!("{n}")),
                    _ => None,
                };
                cells.push((cell_ref(r as u32 + 1, *cnum), write));
            }
        }
        if !cells.is_empty() {
            edits.push(SheetEdit {
                sheet: g.sheet.clone(),
                cells,
            });
        }
    }
    Ok(edits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheme::{ColumnSpec, ColumnType, FieldExpr, ParseSpec, SourceSpec};
    use calamine::Reader;

    fn qingjia_source() -> SourceSpec {
        SourceSpec {
            id: "qingjia".into(),
            file: "请假替上课.xlsx".into(),
            sheet: "请假替上课".into(),
            header_row: 1,
            parse: ParseSpec {
                fill_down: vec!["姓名".into()],
                continue_record_on: vec![],
            },
            columns: vec![
                ColumnSpec {
                    header: Some("姓名".into()),
                    index: None,
                    alias: "姓名".into(),
                    value_type: ColumnType::Text,
                    key: true,
                    required: false,
                    bits: None,
                },
                ColumnSpec {
                    header: Some("类型".into()),
                    index: None,
                    alias: "类型".into(),
                    value_type: ColumnType::Text,
                    key: false,
                    required: false,
                    bits: None,
                },
                ColumnSpec {
                    header: Some("天数".into()),
                    index: None,
                    alias: "天数".into(),
                    value_type: ColumnType::Decimal,
                    key: false,
                    required: false,
                    bits: None,
                },
                ColumnSpec {
                    header: Some("替上课老师".into()),
                    index: None,
                    alias: "替上课老师".into(),
                    value_type: ColumnType::Text,
                    key: false,
                    required: false,
                    bits: None,
                },
                ColumnSpec {
                    header: Some("节数".into()),
                    index: None,
                    alias: "节数".into(),
                    value_type: ColumnType::Integer,
                    key: false,
                    required: false,
                    bits: None,
                },
            ],
        }
    }

    fn bukeshi_derived() -> DerivedSpec {
        DerivedSpec {
            id: "bukeshi_agg".into(),
            from: "qingjia".into(),
            merge: None,
            when: Some("qingjia.替上课老师 != ''".into()),
            columns: vec![
                crate::scheme::ColumnDef {
                    name: "姓名".into(),
                    expr: Some("GROUP_BY(qingjia.替上课老师)".into()),
                },
                crate::scheme::ColumnDef {
                    name: "节数".into(),
                    expr: Some("SUM(qingjia.节数)".into()),
                },
                crate::scheme::ColumnDef {
                    name: "费用".into(),
                    expr: Some("bukeshi_agg.节数 * 参数.补课时费".into()),
                },
            ],
        }
    }

    #[test]
    fn derive_bukeshi_matches_golden() {
        let dir = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../testdata/2023-09"));
        let scheme = Scheme {
            parameters: vec![crate::scheme::Parameter {
                name: "补课时费".into(),
                label: Some("补课时费".into()),
                value_type: crate::scheme::ColumnType::Decimal,
                value: crate::scheme::ParamValue::Integer(7),
            }],
            sources: vec![qingjia_source()],
            derived: vec![bukeshi_derived()],
            ..Default::default()
        };

        let run = run_scheme(&scheme, dir).expect("执行方案失败");
        let table = run.derived_tables.get("bukeshi_agg").expect("应有派生表");
        assert_eq!(table.columns, vec!["姓名", "节数", "费用"]);

        // 引擎结果：姓名 → (节数, 费用)
        let mut got: HashMap<String, (f64, f64)> = HashMap::new();
        for row in &table.rows {
            let name = match row.get("姓名") {
                Some(Value::Str(s)) => s.clone(),
                _ => continue,
            };
            let jie = match row.get("节数") {
                Some(Value::Num(n)) => *n,
                _ => continue,
            };
            let fee = match row.get("费用") {
                Some(Value::Num(n)) => *n,
                _ => continue,
            };
            got.insert(name, (jie, fee));
        }

        // golden：testdata/2023-09-golden/bukeshi/补课时.xlsx（旧程序产物，行序随机，按姓名比对）
        let golden_path = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../testdata/2023-09-golden/bukeshi/补课时.xlsx"
        ));
        let mut wb = calamine::open_workbook_auto(golden_path).expect("打开 golden 失败");
        let range = wb
            .worksheet_range("替上课补课时")
            .expect("读取 golden sheet 失败");
        let (nrows, ncols) = range.get_size();
        assert!(ncols >= 4, "golden 应有 4 列");
        let mut want: HashMap<String, (f64, f64)> = HashMap::new();
        for r in 1..nrows {
            let name = match range.get_value((r as u32, 1)) {
                Some(calamine::Data::String(s)) => s.clone(),
                _ => continue,
            };
            let jie = match range.get_value((r as u32, 2)) {
                Some(d) => calamine_to_f64(d),
                _ => continue,
            };
            let fee = match range.get_value((r as u32, 3)) {
                Some(d) => calamine_to_f64(d),
                _ => continue,
            };
            want.insert(name, (jie, fee));
        }

        assert_eq!(
            got.len(),
            want.len(),
            "人数不一致: got={} want={}",
            got.len(),
            want.len()
        );
        for (name, (gj, gf)) in &got {
            let (wj, wf) = want
                .get(name)
                .unwrap_or_else(|| panic!("golden 缺少 {name}"));
            assert!(
                (gj - wj).abs() < 1e-6,
                "{name} 节数不一致: got={gj} want={wj}"
            );
            assert!(
                (gf - wf).abs() < 1e-6,
                "{name} 费用不一致: got={gf} want={wf}"
            );
        }
    }

    fn calamine_to_f64(d: &calamine::Data) -> f64 {
        match d {
            calamine::Data::Int(i) => *i as f64,
            calamine::Data::Float(f) => *f,
            _ => 0.0,
        }
    }

    #[test]
    fn aggregates_and_group_fields() {
        use crate::scheme::{AggregateSpec, GroupSpec, ParamValue, Parameter};
        use crate::table::{Column as TColumn, Row as TRow, Table as TTable};

        let keshi = TTable {
            columns: vec![
                TColumn {
                    alias: "姓名".into(),
                    header: "姓名".into(),
                },
                TColumn {
                    alias: "课时".into(),
                    header: "课时".into(),
                },
            ],
            rows: vec![TRow {
                cells: vec![CellValue::Text("张".into()), CellValue::Float(20.0)],
            }],
        };
        let qingjia = TTable {
            columns: vec![
                TColumn {
                    alias: "姓名".into(),
                    header: "姓名".into(),
                },
                TColumn {
                    alias: "类型".into(),
                    header: "类型".into(),
                },
                TColumn {
                    alias: "天数".into(),
                    header: "天数".into(),
                },
            ],
            rows: vec![
                TRow {
                    cells: vec![
                        CellValue::Text("张".into()),
                        CellValue::Text("事".into()),
                        CellValue::Float(2.0),
                    ],
                },
                TRow {
                    cells: vec![
                        CellValue::Text("张".into()),
                        CellValue::Text("公".into()),
                        CellValue::Float(3.0),
                    ],
                },
            ],
        };
        let run = SchemeRun {
            source_tables: HashMap::from([
                ("keshi".to_string(), keshi),
                ("qingjia".to_string(), qingjia),
            ]),
            derived_tables: HashMap::new(),
        };

        let mut scheme = Scheme::default();
        scheme.entity.key_field = "姓名".into();
        scheme.parameters = vec![
            Parameter {
                name: "请假超过天数".into(),
                label: None,
                value_type: ColumnType::Integer,
                value: ParamValue::Integer(2),
            },
            Parameter {
                name: "课时费".into(),
                label: None,
                value_type: ColumnType::Decimal,
                value: ParamValue::Float(1.5),
            },
        ];
        scheme.aggregates = vec![
            AggregateSpec {
                id: "请假超总天数".into(),
                from: "qingjia".into(),
                group_by: "姓名".into(),
                expr: "SUM(天数 WHERE 类型 != '公')".into(),
            },
            AggregateSpec {
                id: "请假超额".into(),
                from: "qingjia".into(),
                group_by: "姓名".into(),
                expr: "请假超总天数 >= 参数.请假超过天数".into(),
            },
        ];
        scheme.groups = vec![GroupSpec {
            id: "fxp".into(),
            name: "非校聘".into(),
            match_sheet: None,
            fields: vec![
                FieldExpr {
                    name: "课时S".into(),
                    expr: Some("keshi.课时 ?? 0".into()),
                },
                FieldExpr {
                    name: "课时费".into(),
                    expr: Some("课时S * 参数.课时费".into()),
                },
                FieldExpr {
                    name: "发放".into(),
                    expr: Some("IF(请假超额, 0, 课时费)".into()),
                },
            ],
            overrides: vec![],
        }];

        let aggs = compute_aggregates(&scheme, &run).expect("聚合失败");
        assert_eq!(aggs["请假超总天数"]["张"], Value::Num(2.0));
        assert_eq!(aggs["请假超额"]["张"], Value::Bool(true));

        let fields =
            compute_group_fields_for(&scheme, &run, &aggs, "fxp", "张").expect("字段求值失败");
        assert_eq!(fields["课时S"], Value::Num(20.0));
        assert_eq!(fields["课时费"], Value::Num(30.0));
        assert_eq!(fields["发放"], Value::Num(0.0));
    }

    // ---- 绩效复刻整链：与 golden/kpi/绩效.xlsx 逐格对拍 ----

    fn read_value_grid(path: &Path, sheet: &str) -> std::collections::BTreeMap<(u32, u32), String> {
        use calamine::Data;
        let mut wb = calamine::open_workbook_auto(path).expect("打开工作簿");
        let range = wb.worksheet_range(sheet).expect("读取 sheet");
        let (rows, cols) = range.get_size();
        let mut m = std::collections::BTreeMap::new();
        for r in 0..rows {
            for c in 0..cols {
                let key = (r as u32, c as u32);
                let v = match range.get_value(key) {
                    Some(Data::Float(f)) => format!("n{f}"),
                    Some(Data::Int(i)) => format!("n{i}.0"),
                    Some(Data::String(s)) if !s.is_empty() => format!("s{s}"),
                    // 空字符串单元格与缺失/空单元格等价
                    Some(Data::String(_)) | Some(Data::Empty) | None => "e".into(),
                    Some(Data::Bool(b)) => format!("b{b}"),
                    _ => "e".into(),
                };
                m.insert(key, v);
            }
        }
        m
    }

    fn grid_equal(a: &str, b: &str) -> bool {
        match (a.strip_prefix('n'), b.strip_prefix('n')) {
            (Some(x), Some(y)) => {
                let x: f64 = x.parse().unwrap_or(f64::NAN);
                let y: f64 = y.parse().unwrap_or(f64::NAN);
                x == y
            }
            _ => a == b,
        }
    }

    fn count_grid_diffs(path_a: &Path, path_b: &Path) -> usize {
        let wb = calamine::open_workbook_auto(path_b).expect("打开基准");
        let mut ndiff = 0usize;
        for sheet in wb.sheet_names().iter() {
            if sheet != "非校聘" && sheet != "校聘" {
                continue;
            }
            let ma = read_value_grid(path_a, sheet);
            let mb = read_value_grid(path_b, sheet);
            let mut keys: Vec<(u32, u32)> = ma.keys().chain(mb.keys()).copied().collect();
            keys.sort_unstable();
            keys.dedup();
            for k in keys {
                let va = ma.get(&k).cloned().unwrap_or_else(|| "e".into());
                let vb = mb.get(&k).cloned().unwrap_or_else(|| "e".into());
                if !grid_equal(&va, &vb) {
                    ndiff += 1;
                }
            }
        }
        ndiff
    }

    #[test]
    fn kpi_replication_matches_golden() {
        let scheme_path = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../schemes/高中月绩效复刻.json"
        ));
        let raw = std::fs::read_to_string(scheme_path).expect("读复刻方案失败");
        let v = crate::scheme::VersionedScheme::from_json_str(&raw).expect("复刻方案解析失败");
        let scheme = v.scheme;

        let dir = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../testdata/2023-09"));
        let run = run_scheme(&scheme, dir).expect("执行 sources/derived 失败");
        let aggs = compute_aggregates(&scheme, &run).expect("聚合失败");
        let edits = plan_template_patch(&scheme, &run, &aggs, "jx", dir).expect("回写规划失败");
        assert!(
            edits.iter().any(|e| !e.cells.is_empty()),
            "应规划出非空编辑"
        );

        let golden = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../testdata/2023-09-golden/kpi/绩效.xlsx"
        ));
        let template = dir.join("绩效.xlsx");
        let out_path =
            std::env::temp_dir().join(format!("kpi-repl-test-{}.xlsx", std::process::id()));
        crate::template::patch_workbook(&template, &out_path, &edits).expect("模板回写失败");

        let diffs = count_grid_diffs(&out_path, golden);
        let _ = std::fs::remove_file(&out_path);
        assert_eq!(diffs, 0, "与 golden 仍有 {diffs} 个差异单元格");
    }

    /// 读取补课时文件：按表头定位 姓名/节数/费用 列，返回 姓名 → (节数, 费用)。
    fn read_bukeshi_by_name(path: &Path) -> std::collections::HashMap<String, (f64, f64)> {
        use calamine::{Data, Reader};
        let mut wb = calamine::open_workbook_auto(path).expect("打开补课时文件");
        let range = wb.worksheet_range("替上课补课时").expect("读 sheet");
        let (rows, cols) = range.get_size();
        let mut col_of = std::collections::HashMap::new();
        for c in 0..cols {
            if let Some(Data::String(s)) = range.get_value((0, c as u32)) {
                col_of.insert(s.clone(), c as u32);
            }
        }
        let to_f = |d: Option<&Data>| -> f64 {
            match d {
                Some(Data::Int(i)) => *i as f64,
                Some(Data::Float(f)) => *f,
                _ => 0.0,
            }
        };
        let mut out = std::collections::HashMap::new();
        for r in 1..rows {
            if let Some(Data::String(name)) =
                range.get_value((r as u32, *col_of.get("姓名").expect("无姓名列")))
            {
                let jie = to_f(range.get_value((r as u32, *col_of.get("节数").expect("无节数列"))));
                let fee = to_f(range.get_value((r as u32, *col_of.get("费用").expect("无费用列"))));
                out.insert(name.clone(), (jie, fee));
            }
        }
        out
    }

    #[test]
    fn bukeshi_new_file_matches_golden() {
        use crate::scheme::{NewColumn, NewFileOutput, OutputKind, OutputSpec};
        let scheme_path = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../schemes/高中月绩效复刻.json"
        ));
        let raw = std::fs::read_to_string(scheme_path).expect("读复刻方案失败");
        let mut v = crate::scheme::VersionedScheme::from_json_str(&raw).expect("复刻方案解析失败");
        v.scheme.outputs.push(OutputSpec {
            id: "补课时新表".into(),
            kind: OutputKind::NewFile(NewFileOutput {
                file: "{monthDir}/补课时.xlsx".into(),
                from: "bukesh".into(),
                sheet: "替上课补课时".into(),
                columns: vec![
                    NewColumn {
                        header: "姓名".into(),
                        field: "姓名".into(),
                    },
                    NewColumn {
                        header: "节数".into(),
                        field: "节数".into(),
                    },
                    NewColumn {
                        header: "费用".into(),
                        field: "费用".into(),
                    },
                ],
            }),
        });

        let dir = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../testdata/2023-09"));
        let run = run_scheme(&v.scheme, dir).expect("执行失败");
        let table = run
            .derived_tables
            .get("bukesh")
            .expect("应有 bukesh 派生表");
        let out_path =
            std::env::temp_dir().join(format!("bukeshi-repl-{}.xlsx", std::process::id()));
        let spec = v
            .scheme
            .outputs
            .iter()
            .find(|o| o.id == "补课时新表")
            .map(|o| match &o.kind {
                OutputKind::NewFile(n) => n,
                _ => unreachable!(),
            })
            .expect("应有新文件输出");
        crate::writer::write_new_file(&out_path, spec, table).expect("写新文件失败");

        let golden = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../testdata/2023-09-golden/bukeshi/补课时.xlsx"
        ));
        let got = read_bukeshi_by_name(&out_path);
        let want = read_bukeshi_by_name(golden);
        let _ = std::fs::remove_file(&out_path);
        assert_eq!(got.len(), want.len(), "人数不一致");
        for (name, (gj, gf)) in &got {
            let (wj, wf) = want
                .get(name)
                .unwrap_or_else(|| panic!("golden 缺少 {name}"));
            assert_eq!(gj, wj, "{name} 节数不一致");
            assert_eq!(gf, wf, "{name} 费用不一致");
        }
    }

    #[test]
    fn merge_view_combines_continuation_rows() {
        use std::collections::HashMap;
        let lead = HashMap::from([
            ("姓名".to_string(), Value::Str("张".into())),
            ("类型".to_string(), Value::Str("事".into())),
            ("天数".to_string(), Value::Num(2.0)),
            ("节数".to_string(), Value::Num(0.0)),
        ]);
        let cont = HashMap::from([
            ("姓名".to_string(), Value::Str("张".into())),
            ("天数".to_string(), Value::Empty),
            ("节数".to_string(), Value::Num(3.0)),
        ]);
        let cont2 = HashMap::from([
            ("姓名".to_string(), Value::Str("张".into())),
            ("天数".to_string(), Value::Empty),
            ("节数".to_string(), Value::Num(2.0)),
        ]);
        let next_lead = HashMap::from([
            ("姓名".to_string(), Value::Str("李".into())),
            ("类型".to_string(), Value::Str("病".into())),
            ("天数".to_string(), Value::Num(1.0)),
            ("节数".to_string(), Value::Num(0.0)),
        ]);
        let rows = vec![lead, cont, cont2, next_lead];
        let spec = DerivedSpec {
            id: "view".into(),
            from: "x".into(),
            merge: Some(crate::scheme::MergeSpec {
                on: vec!["天数".into()],
            }),
            when: None,
            columns: vec![],
        };
        let cols = vec![
            "姓名".to_string(),
            "类型".to_string(),
            "天数".to_string(),
            "节数".to_string(),
        ];
        let table = compute_derived(&spec, &HashMap::new(), &cols, &rows).expect("merge 派生失败");
        // 前两行续行并入主记录 → 2 条记录
        assert_eq!(table.rows.len(), 2);
        let first = &table.rows[0];
        assert_eq!(first.get("天数"), Some(&Value::Num(2.0)));
        assert_eq!(first.get("节数"), Some(&Value::Num(5.0)));
        // 行视图透传原始列
        assert_eq!(table.columns, cols);
    }
}
