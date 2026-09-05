//! exec：方案执行器内核（M1-1.3 起点）。
//!
//! 当前范围：把 scheme 的 sources 读成规整表，并执行 **derived（派生明细）**：
//!   行过滤(when) → 列重映射(map) → 按 group_by 分组 → 计算 fields（支持聚合表达式）。
//! 下一步再接 aggregates（按人聚合）与 groups 字段求值 / outputs。

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

/// 把方案里的 `{monthDir}` 绑定为实际目录。
pub fn resolve_file(pattern: &str, dir: &Path) -> PathBuf {
    dir.join(pattern.replace("{monthDir}", "."))
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
    for d in &scheme.derived {
        let rows = rows_for(&d.from, &source_tables, &derived_tables)?;
        let table = compute_derived(d, &rows)?;
        derived_tables.insert(d.id.clone(), table);
    }

    Ok(SchemeRun {
        source_tables,
        derived_tables,
    })
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

/// 执行派生：过滤 → 重映射 → 分组 → 字段计算。
pub fn compute_derived(spec: &DerivedSpec, in_rows: &[CalcRow]) -> Result<DerivedTable> {
    // 1) 过滤
    let when_node = spec
        .when
        .as_deref()
        .map(calc::parse)
        .transpose()
        .map_err(|e| Error::Runtime(format!("derived.{} when 解析失败: {e}", spec.id)))?;
    let mut rows: Vec<CalcRow> = Vec::new();
    for r in in_rows {
        let keep = match &when_node {
            Some(n) => {
                let v = calc::eval_on_row(n, &|_| None, r).map_err(|e| {
                    Error::Runtime(format!("derived.{} when 求值失败: {e}", spec.id))
                })?;
                matches!(v, Value::Bool(true)) || matches!(v, Value::Num(x) if x != 0.0)
            }
            None => true,
        };
        if keep {
            rows.push(r.clone());
        }
    }

    // 2) 重映射：目标 alias ← 源列 alias
    let mut mapped: Vec<CalcRow> = Vec::new();
    for r in &rows {
        let mut m = CalcRow::new();
        for (to, from) in &spec.map {
            let v = r.get(from).cloned().unwrap_or(Value::Empty);
            m.insert(to.clone(), v);
        }
        mapped.push(m);
    }

    // 3) 分组 + 字段
    let mut out_rows: Vec<CalcRow> = Vec::new();
    let group_key = spec.group_by.clone();
    let compute = |group_rows: &[CalcRow]| -> Result<CalcRow> {
        let mut out = CalcRow::new();
        if let Some(gb) = &group_key
            && let Some(first) = group_rows.first()
        {
            out.insert(gb.clone(), first.get(gb).cloned().unwrap_or(Value::Empty));
        }
        let mut comp: HashMap<String, Value> = HashMap::new();
        for f in &spec.fields {
            let expr_text = f.expr.as_deref().ok_or_else(|| {
                Error::Runtime(format!("derived.{} 字段 {} 缺表达式", spec.id, f.name))
            })?;
            let node = calc::parse(expr_text).map_err(|e| {
                Error::Runtime(format!("derived.{} 字段 {} 解析失败: {e}", spec.id, f.name))
            })?;
            let val = eval_field(&node, group_rows, &comp)?;
            comp.insert(f.name.clone(), val.clone());
            out.insert(f.name.clone(), val);
        }
        Ok(out)
    };

    if let Some(gb) = &group_key {
        let mut groups: BTreeMap<String, Vec<CalcRow>> = BTreeMap::new();
        for r in &mapped {
            let k = value_key(r.get(gb).unwrap_or(&Value::Empty));
            groups.entry(k).or_default().push(r.clone());
        }
        for (_k, g) in groups {
            out_rows.push(compute(&g)?);
        }
    } else {
        for r in &mapped {
            out_rows.push(compute(std::slice::from_ref(r))?);
        }
    }

    // 4) 列顺序：group 列 + 计算字段
    let mut columns = Vec::new();
    if let Some(gb) = &group_key {
        columns.push(gb.clone());
    }
    columns.extend(spec.fields.iter().map(|f| f.name.clone()));

    Ok(DerivedTable {
        columns,
        rows: out_rows,
    })
}

fn eval_field(node: &Node, group_rows: &[CalcRow], comp: &HashMap<String, Value>) -> Result<Value> {
    calc::eval_rows(
        node,
        &|p| {
            if p.len() == 1 {
                if let Some(v) = comp.get(&p[0]) {
                    return Some(v.clone());
                }
                if let Some(first) = group_rows.first()
                    && let Some(v) = first.get(&p[0])
                {
                    return Some(v.clone());
                }
            }
            None
        },
        group_rows,
    )
    .map_err(Error::Runtime)
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

    // 领域函数（按人统计）：扣课时/请假/折扣等旧语义的 1:1 复刻。
    let extra = |name: &str, args: &[Value]| Ok(special_value(name, args, scheme, run, person));

    for (name, expr_text) in fields {
        let Some(text) = expr_text else {
            continue;
        };
        let node =
            calc::parse(&text).map_err(|e| Error::Runtime(format!("字段 {name} 解析失败: {e}")))?;
        let value = calc::eval_with(
            &node,
            &|p| {
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
            },
            Some(&extra),
        )
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

// ---- 绩效复刻的"按人统计"领域函数（LEAVE_DEDUCT / LEAVE_PENALTY / DISCOUNT）----
// 旧逻辑见 school-kpi collect/collect.go 的 CalKks / CalQj / CalZk。
// 这些函数以"请假明细表"（scheme.aggregates[0].from）逐人重建请假记录来求值，
// 属于 legacy 1:1 语义（R3 疑点仍按旧行为复刻）。

/// 一条请假记录（主记录 + 其下若干替课行）。
struct LeaveRecord {
    jia_type: Option<String>,
    days: f64,
    has_sub: bool,
}

fn leave_table_id(scheme: &Scheme) -> Option<String> {
    scheme.aggregates.first().map(|a| a.from.clone())
}

fn val_num(v: &Value) -> f64 {
    match v {
        Value::Num(n) => *n,
        Value::Bool(b) => {
            if *b {
                1.0
            } else {
                0.0
            }
        }
        Value::Str(s) => s.trim().parse().unwrap_or(0.0),
        Value::Empty => 0.0,
    }
}

fn val_bool(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Num(n) => *n != 0.0,
        Value::Str(s) => !s.is_empty(),
        Value::Empty => false,
    }
}

fn row_has_sub(r: &CalcRow) -> bool {
    matches!(
        r.get("替上课老师"),
        Some(Value::Str(s)) if !s.trim().is_empty()
    )
}

/// 重建某人的请假记录（按"天数"列断行，替课行续到前一条记录）。
fn person_leave_records(scheme: &Scheme, run: &SchemeRun, person: &str) -> Vec<LeaveRecord> {
    let Some(id) = leave_table_id(scheme) else {
        return Vec::new();
    };
    let rows: Vec<CalcRow> = match (run.source_tables.get(&id), run.derived_tables.get(&id)) {
        (Some(t), _) => table_to_rows(t),
        (None, Some(d)) => d.rows.clone(),
        (None, None) => Vec::new(),
    };
    let key = scheme.entity.key_field.clone();
    let mut out: Vec<LeaveRecord> = Vec::new();
    let mut cur: Option<LeaveRecord> = None;
    let want = normalize_key(person);
    for r in rows {
        let k = key_value(&r, &key)
            .map(|v| normalize_key(&value_key(v)))
            .unwrap_or_default();
        if k != want {
            continue;
        }
        let days = match r.get("天数") {
            Some(Value::Num(n)) => Some(*n),
            _ => None,
        };
        match days {
            Some(d) => {
                if let Some(prev) = cur.take() {
                    out.push(prev);
                }
                let jia_type = match r.get("类型") {
                    Some(Value::Str(s)) => Some(s.clone()),
                    _ => None,
                };
                cur = Some(LeaveRecord {
                    jia_type,
                    days: d,
                    has_sub: row_has_sub(&r),
                });
            }
            None => {
                if let Some(rec) = cur.as_mut()
                    && row_has_sub(&r)
                {
                    rec.has_sub = true;
                }
            }
        }
    }
    if let Some(rec) = cur {
        out.push(rec);
    }
    out
}

/// LEAVE_DEDUCT(课时周费)：扣课时 = -(Σ 有替课且类型∉{婚,公,丧,产} 的记录天数 × 周费/5)。
/// 无请假/结果 0 时返回 空（旧代码 ok=false → 留空）。
fn leave_deduct_value(scheme: &Scheme, run: &SchemeRun, person: &str, kszf: f64) -> Value {
    let records = person_leave_records(scheme, run, person);
    if records.is_empty() {
        return Value::Empty;
    }
    let excluded = ["婚", "公", "丧", "产"];
    let per_day = kszf / 5.0;
    let mut total = 0.0f64;
    for r in &records {
        if !r.has_sub {
            continue;
        }
        match r.jia_type.as_deref() {
            Some(t) if !excluded.contains(&t) => total += per_day * r.days,
            _ => {}
        }
    }
    if total == 0.0 {
        Value::Empty
    } else {
        Value::Num(-total)
    }
}

/// LEAVE_PENALTY()：请假扣款 = -MIN(100, 事假天数×单价 + 病假天数×单价)；0 → 空。
fn leave_penalty_value(scheme: &Scheme, run: &SchemeRun, person: &str) -> Value {
    let records = person_leave_records(scheme, run, person);
    let params = param_lookup(scheme);
    let shijia = val_num(params.get("事假单价").unwrap_or(&Value::Num(0.0)));
    let bingjia = val_num(params.get("病假单价").unwrap_or(&Value::Num(0.0)));
    let mut money = 0.0f64;
    for r in &records {
        match r.jia_type.as_deref() {
            Some("事") => money += r.days * shijia,
            Some("病") => money += r.days * bingjia,
            _ => {}
        }
    }
    if money == 0.0 {
        Value::Empty
    } else {
        Value::Num(-money.min(100.0))
    }
}

/// DISCOUNT(教研组长, 年级组长, 行政后勤, 班主任, 课时月费)。
/// 按旧 CalZk 复刻：职位 >0 数量 ≥2 时取"最后一个非零值"×折扣；否则仅当参数开启时
/// MIN(课时月费, 行政后勤)×折扣，默认 0。
fn discount_value(args: &[Value], scheme: &Scheme) -> Value {
    if args.len() < 5 {
        return Value::Num(0.0);
    }
    let ms = [
        val_num(&args[0]),
        val_num(&args[1]),
        val_num(&args[2]),
        val_num(&args[3]),
    ];
    let ksyf = val_num(&args[4]);
    let params = param_lookup(scheme);
    let zk = val_num(params.get("折扣").unwrap_or(&Value::Num(0.0)));
    let enable = val_bool(
        params
            .get("是否对课时月费进行折扣")
            .unwrap_or(&Value::Bool(false)),
    );
    let count = ms.iter().filter(|&&v| v > 0.0).count();
    let v = if count >= 2 {
        // 旧 min() 的"取最后一个非零值"（R3 疑点，按旧行为复刻）。
        let last = ms
            .iter()
            .fold(0.0f64, |acc, &v| if v != 0.0 { v } else { acc });
        last * zk
    } else if enable {
        ksyf.min(ms[2]) * zk
    } else {
        0.0
    };
    Value::Num(v)
}

/// 领域函数分发（不认识返回 None，走默认"未知函数"报错）。
fn special_value(
    name: &str,
    args: &[Value],
    scheme: &Scheme,
    run: &SchemeRun,
    person: &str,
) -> Option<Value> {
    let up = name.to_uppercase();
    match up.as_str() {
        "LEAVE_DEDUCT" if !args.is_empty() => {
            Some(leave_deduct_value(scheme, run, person, val_num(&args[0])))
        }
        "LEAVE_PENALTY" => Some(leave_penalty_value(scheme, run, person)),
        "DISCOUNT" => Some(discount_value(args, scheme)),
        _ => None,
    }
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
            label: None,
            file: "{monthDir}/请假替上课.xlsx".into(),
            sheet: "请假替上课".into(),
            header_row: 1,
            parse: ParseSpec {
                fill_down: vec!["姓名".into()],
                continue_record_on: vec!["天数".into()],
            },
            columns: vec![
                ColumnSpec {
                    header: Some("姓名".into()),
                    index: None,
                    alias: "姓名".into(),
                    value_type: ColumnType::Text,
                    key: true,
                    required: false,
                    normalize: vec![],
                    bits: None,
                },
                ColumnSpec {
                    header: Some("类型".into()),
                    index: None,
                    alias: "类型".into(),
                    value_type: ColumnType::Text,
                    key: false,
                    required: false,
                    normalize: vec![],
                    bits: None,
                },
                ColumnSpec {
                    header: Some("天数".into()),
                    index: None,
                    alias: "天数".into(),
                    value_type: ColumnType::Decimal,
                    key: false,
                    required: false,
                    normalize: vec![],
                    bits: None,
                },
                ColumnSpec {
                    header: Some("替上课老师".into()),
                    index: None,
                    alias: "替上课老师".into(),
                    value_type: ColumnType::Text,
                    key: false,
                    required: false,
                    normalize: vec![],
                    bits: None,
                },
                ColumnSpec {
                    header: Some("节数".into()),
                    index: None,
                    alias: "节数".into(),
                    value_type: ColumnType::Integer,
                    key: false,
                    required: false,
                    normalize: vec![],
                    bits: None,
                },
            ],
        }
    }

    fn bukeshi_derived() -> DerivedSpec {
        DerivedSpec {
            id: "bukeshi_agg".into(),
            from: "qingjia".into(),
            when: Some("替上课老师 != ''".into()),
            map: BTreeMap::from([
                ("姓名".to_string(), "替上课老师".to_string()),
                ("节数".to_string(), "节数".to_string()),
            ]),
            group_by: Some("姓名".into()),
            fields: vec![
                FieldExpr {
                    name: "节数".into(),
                    expr: Some("SUM(节数)".into()),
                },
                FieldExpr {
                    name: "费用".into(),
                    expr: Some("节数 * 7".into()),
                },
            ],
        }
    }

    #[test]
    fn derive_bukeshi_matches_golden() {
        let dir = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../testdata/2023-09"));
        let scheme = Scheme {
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
}
