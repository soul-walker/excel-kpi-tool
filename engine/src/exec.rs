//! exec：方案执行器内核（M1-1.3 起点）。
//!
//! 当前范围：把 scheme 的 sources 读成规整表，并执行 **derived（派生明细）**：
//!   行过滤(when) → 列重映射(map) → 按 group_by 分组 → 计算 fields（支持聚合表达式）。
//! 下一步再接 aggregates（按人聚合）与 groups 字段求值 / outputs。

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use crate::calc::{self, Node, Row as CalcRow, Value};
use crate::error::{Error, Result};
use crate::reader::read_source_table;
use crate::scheme::{DerivedSpec, Scheme};
use crate::table::{CellValue, Table};

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
                },
                ColumnSpec {
                    header: Some("类型".into()),
                    index: None,
                    alias: "类型".into(),
                    value_type: ColumnType::Text,
                    key: false,
                    required: false,
                    normalize: vec![],
                },
                ColumnSpec {
                    header: Some("天数".into()),
                    index: None,
                    alias: "天数".into(),
                    value_type: ColumnType::Decimal,
                    key: false,
                    required: false,
                    normalize: vec![],
                },
                ColumnSpec {
                    header: Some("替上课老师".into()),
                    index: None,
                    alias: "替上课老师".into(),
                    value_type: ColumnType::Text,
                    key: false,
                    required: false,
                    normalize: vec![],
                },
                ColumnSpec {
                    header: Some("节数".into()),
                    index: None,
                    alias: "节数".into(),
                    value_type: ColumnType::Integer,
                    key: false,
                    required: false,
                    normalize: vec![],
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
}
