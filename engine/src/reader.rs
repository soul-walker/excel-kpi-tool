//! 模板/台账"定位读取"（只读所需最小信息，不做整簿重建）。
//!
//! 读取端选定 calamine（M0 已验证：能正确读出 umya 静默失败的 绩效/补课时 等文件）。
//! 这里只回答三个问题：
//!   1) 表头在哪一行？（支持"行内含全部声明列名"——适配 标题行/多行表头）
//!   2) 每个期望表头在第几列？
//!   3) key 列（如 姓名）的值落在第几行？

use std::collections::BTreeMap;
use std::path::Path;

use calamine::{Data, Reader, open_workbook_auto};

use crate::error::{Error, Result};

/// 定位结果（行/列均为 Excel 1 起始）。
#[derive(Debug, Clone)]
pub struct LocatedSheet {
    pub sheet: String,
    /// 表头所在 Excel 行号（1 起始）。
    pub header_row: u32,
    /// 期望表头名 → 列号（1 起始）。
    pub col_of: BTreeMap<String, u32>,
    /// (归一化 key 值, Excel 行号)，保持表内出现顺序。
    pub key_rows: Vec<(String, u32)>,
}

/// 简单的展示归一化（去空白/换行/全角空格）。
pub fn normalize_key(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace() && *c != '\u{3000}')
        .collect()
}

fn cell_text(d: &Data) -> Option<String> {
    match d {
        Data::String(s) => Some(s.clone()),
        Data::Int(i) => Some(i.to_string()),
        Data::Float(f) => Some(f.to_string()),
        Data::DateTime(dt) => Some(dt.to_string()),
        Data::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// 定位某 sheet：找到"包含全部期望表头"的行，并给出 key 列的值→行映射。
///
/// `expect_headers` 顺序无关；表头行是第一个包含全部表头名（按去空格比较）的行。
/// `key_header` 必须是 `expect_headers` 之一。
pub fn locate_sheet(
    path: &Path,
    sheet: &str,
    expect_headers: &[&str],
    key_header: &str,
) -> Result<LocatedSheet> {
    let mut wb =
        open_workbook_auto(path).map_err(|e| Error::Runtime(format!("打开 {path:?} 失败: {e}")))?;
    if !wb.sheet_names().iter().any(|n| n == sheet) {
        return Err(Error::Runtime(format!("工作簿缺少 sheet: {sheet}")));
    }
    let range = wb
        .worksheet_range(sheet)
        .map_err(|e| Error::Runtime(format!("读取 sheet {sheet} 失败: {e}")))?;
    let (rows, cols) = range.get_size();

    let norm_expected: Vec<String> = expect_headers.iter().map(|h| normalize_key(h)).collect();
    let mut header_row: Option<u32> = None;
    let mut col_of: BTreeMap<String, u32> = BTreeMap::new();
    let mut key_rows: Vec<(String, u32)> = Vec::new();
    let mut key_col: Option<u32> = None;

    for r in 0..rows {
        // —— 找表头行 ——
        if header_row.is_none() {
            let mut found = BTreeMap::new();
            for c in 0..cols {
                if let Some(Data::String(text)) = range.get_value((r as u32, c as u32)) {
                    let key = normalize_key(text);
                    if norm_expected.iter().any(|h| h == &key) {
                        found
                            .entry(key.clone())
                            .or_insert((c as u32 + 1, text.clone()));
                    }
                }
            }
            if norm_expected.iter().all(|h| found.contains_key(h)) {
                header_row = Some(r as u32 + 1);
                for h in &norm_expected {
                    let (col, raw) = found.get(h).expect("已检查存在");
                    col_of.insert(raw.clone(), *col);
                }
                // key 列号记录原始表头名对应列
                let raw_key = found
                    .get(&normalize_key(key_header))
                    .map(|(_, s)| s.clone());
                key_col = raw_key.as_ref().and_then(|raw| col_of.get(raw).copied());
            }
            continue;
        }
        // —— 数据行：key → 行 ——
        let c = match key_col {
            Some(c) => c,
            None => break,
        };
        if let Some(text) = range.get_value((r as u32, c - 1)).and_then(cell_text) {
            let key = normalize_key(&text);
            if !key.is_empty() {
                key_rows.push((key, r as u32 + 1));
            }
        }
    }

    let header_row = header_row.ok_or_else(|| {
        Error::Runtime(format!(
            "sheet {sheet} 未找到同时包含以下表头的行: {}",
            expect_headers.join("、")
        ))
    })?;
    Ok(LocatedSheet {
        sheet: sheet.to_string(),
        header_row,
        col_of,
        key_rows,
    })
}

/// 列号(1 起始) → Excel 列字母，如 1→A、27→AA。
pub fn col_letters(col: u32) -> String {
    let mut n = col;
    let mut s = String::new();
    while n > 0 {
        let rem = (n - 1) % 26;
        s.insert(0, char::from(b'A' + rem as u8));
        n = (n - 1) / 26;
    }
    s
}

/// 单元格引用，如 (3,1) → "A3"。
pub fn cell_ref(row: u32, col: u32) -> String {
    format!("{}{}", col_letters(col), row)
}

// ---- 规整化读取：按方案的数据源/列定义读成 crate::table::Table ----

use crate::scheme::{ColumnSpec, ColumnType, SourceSpec};
use crate::table::{CellValue, Column, Row as TableRow, Table};

/// 按 `src` 的列定义把 sheet 读成规整表格。
/// 表头定位：优先"包含全部声明表头的行"（兼容标题行），否则退回 `src.header_row`。
/// 支持按 `ColumnSpec.value_type` 转换、跳全空行、`parse.fill_down` 向下填充。
pub fn read_source_table(path: &Path, src: &SourceSpec) -> Result<Table> {
    let mut wb =
        open_workbook_auto(path).map_err(|e| Error::Runtime(format!("打开 {path:?} 失败: {e}")))?;
    if !wb.sheet_names().iter().any(|n| n == &src.sheet) {
        return Err(Error::Runtime(format!("sheet 不存在: {}", src.sheet)));
    }
    let range = wb
        .worksheet_range(&src.sheet)
        .map_err(|e| Error::Runtime(format!("读取 sheet {} 失败: {e}", src.sheet)))?;
    let (rows, cols) = range.get_size();

    // —— 定位表头行 ——
    let header_cols: Vec<&ColumnSpec> = src.columns.iter().filter(|c| c.header.is_some()).collect();
    let mut header_idx: Option<usize> = None;
    if !header_cols.is_empty() {
        'outer: for r in 0..rows.min(50) {
            for c in &header_cols {
                let want = normalize_key(c.header.as_deref().unwrap_or(""));
                if want.is_empty() {
                    continue 'outer;
                }
                let found = (0..cols).any(|col| {
                    matches!(
                        range.get_value((r as u32, col as u32)),
                        Some(Data::String(s)) if normalize_key(s) == want
                    )
                });
                if !found {
                    continue 'outer;
                }
            }
            header_idx = Some(r);
            break;
        }
    }

    // 数据起始行（0 起始）：内容定位到表头则其后一行；否则按 src.header_row。
    let data_start = match header_idx {
        Some(r) => r + 1,
        None => src.header_row.saturating_sub(1) as usize,
    };

    // —— 列 alias → range 列索引 ——
    let mut col_alias: BTreeMap<String, usize> = BTreeMap::new();
    let mut col_type: BTreeMap<String, ColumnType> = BTreeMap::new();
    for c in &src.columns {
        let mut idx = None;
        if let Some(h) = header_idx
            && let Some(header) = &c.header
        {
            let want = normalize_key(header);
            for col in 0..cols {
                if let Some(Data::String(s)) = range.get_value((h as u32, col as u32))
                    && normalize_key(s) == want
                {
                    idx = Some(col);
                    break;
                }
            }
        }
        if idx.is_none() {
            idx = c.index.map(|i| (i - 1) as usize);
        }
        let idx = idx.ok_or_else(|| {
            Error::Runtime(format!(
                "列 {} 未定位（既没在表头中找到，也未给 index）",
                c.alias
            ))
        })?;
        if idx >= cols {
            return Err(Error::Runtime(format!("列 {} 超出表宽", c.alias)));
        }
        col_alias.insert(c.alias.clone(), idx);
        col_type.insert(c.alias.clone(), c.value_type);
    }

    // —— 逐行转换 ——
    let mut out_rows: Vec<TableRow> = Vec::new();
    for r in data_start..rows {
        // 先 raw 预检：整行与表头同文（重复表头/小计行）→ 整行跳过
        let mut header_like = false;
        for c in &src.columns {
            if let (Some(Data::String(s)), Some(h)) = (
                range.get_value((r as u32, col_alias[&c.alias] as u32)),
                &c.header,
            ) && normalize_key(s) == normalize_key(h)
            {
                header_like = true;
                break;
            }
        }
        if header_like {
            continue;
        }

        let mut cells = Vec::with_capacity(col_alias.len());
        let mut any = false;
        for c in &src.columns {
            let idx = col_alias[&c.alias];
            let data = range.get_value((r as u32, idx as u32));
            let cell = to_cell(data, c.value_type)?;
            any |= !cell.is_blank();
            cells.push(cell);
        }
        if any {
            out_rows.push(TableRow { cells });
        }
    }

    // —— 向下填充 ——
    if !src.parse.fill_down.is_empty() {
        let mut last: Vec<Option<CellValue>> = vec![None; src.columns.len()];
        for row in &mut out_rows {
            for (i, col) in src.columns.iter().enumerate() {
                if src.parse.fill_down.iter().any(|a| a == &col.alias)
                    && row.cells[i].is_blank()
                    && let Some(v) = &last[i]
                {
                    row.cells[i] = v.clone();
                }
                if !row.cells[i].is_blank() {
                    last[i] = Some(row.cells[i].clone());
                }
            }
        }
    }

    let columns: Vec<Column> = src
        .columns
        .iter()
        .map(|c| Column {
            alias: c.alias.clone(),
            header: c.header.clone().unwrap_or_else(|| c.alias.clone()),
        })
        .collect();
    Ok(Table {
        columns,
        rows: out_rows,
    })
}

fn to_cell(data: Option<&Data>, ct: ColumnType) -> Result<CellValue> {
    let Some(d) = data else {
        return Ok(CellValue::Blank);
    };
    let text = |s: &str| {
        if s.trim().is_empty() {
            CellValue::Blank
        } else {
            CellValue::Text(s.to_string())
        }
    };
    match (d, ct) {
        (Data::String(s), ColumnType::Text) => Ok(text(s)),
        (Data::String(s), ColumnType::Integer) => s
            .trim()
            .parse::<i64>()
            .map(CellValue::Integer)
            .map_err(|_| Error::Runtime(format!("整数解析失败: {s:?}"))),
        (Data::String(s), ColumnType::Decimal) => s
            .trim()
            .parse::<f64>()
            .map(CellValue::Float)
            .map_err(|_| Error::Runtime(format!("小数解析失败: {s:?}"))),
        (Data::String(s), _) => Ok(text(s)),
        (Data::Int(i), ColumnType::Text) => Ok(CellValue::Text(i.to_string())),
        (Data::Int(i), ColumnType::Decimal) => Ok(CellValue::Float(*i as f64)),
        (Data::Int(i), _) => Ok(CellValue::Integer(*i)),
        (Data::Float(f), ColumnType::Integer) if f.fract() == 0.0 => {
            Ok(CellValue::Integer(*f as i64))
        }
        (Data::Float(f), ColumnType::Text) => Ok(CellValue::Text(f.to_string())),
        (Data::Float(f), _) => Ok(CellValue::Float(*f)),
        (Data::Bool(b), ColumnType::Boolean) => Ok(CellValue::Boolean(*b)),
        (Data::Bool(b), _) => Ok(CellValue::Text(b.to_string())),
        (Data::DateTime(dt), _) => Ok(CellValue::Text(dt.to_string())),
        _ => Ok(CellValue::Blank),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheme::{ColumnSpec, ParseSpec, SourceSpec};

    fn keshi_src() -> SourceSpec {
        SourceSpec {
            id: "keshi".into(),
            label: None,
            file: "{monthDir}/课时.xlsx".into(),
            sheet: "课时".into(),
            header_row: 1,
            parse: ParseSpec::default(),
            columns: vec![
                ColumnSpec {
                    header: Some("姓名".into()),
                    index: None,
                    alias: "姓名".into(),
                    value_type: ColumnType::Text,
                    key: true,
                    required: true,
                    normalize: vec![],
                },
                ColumnSpec {
                    header: Some("总计".into()),
                    index: None,
                    alias: "课时".into(),
                    value_type: ColumnType::Decimal,
                    key: false,
                    required: false,
                    normalize: vec![],
                },
            ],
        }
    }

    #[test]
    fn reads_keshi_table() {
        let path = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../testdata/2023-09/课时.xlsx"
        ));
        let table = read_source_table(path, &keshi_src()).expect("应能读取课时表");
        assert!(
            table.row_count() > 50,
            "应读到 50+ 行，实际 {}",
            table.row_count()
        );
        let name_idx = table.column_index("姓名").expect("应有姓名列");
        assert!(
            table
                .rows
                .iter()
                .any(|r| r.cells[name_idx] != CellValue::Blank),
            "姓名列应有内容"
        );
    }
}
