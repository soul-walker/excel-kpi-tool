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
