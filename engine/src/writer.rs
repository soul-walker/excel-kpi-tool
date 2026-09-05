//! writer：**新建文件**输出（create-only 场景，如 补课时.xlsx）。
//!
//! 模板回写不走这里（那是 calamine 定位 + 最小 XML patch，见 reader/template）。
//! rust_xlsxwriter 不能读取既有文件，正好符合"从零生成"的语义。

use std::path::Path;

use rust_xlsxwriter::{Workbook, Worksheet};

use crate::calc::Value;
use crate::error::{Error, Result};
use crate::exec::DerivedTable;
use crate::scheme::NewFileOutput;

/// 把派生表写成新 xlsx（表头来自 `spec.columns`，行值按字段名取）。
pub fn write_new_file(path: &Path, spec: &NewFileOutput, data: &DerivedTable) -> Result<()> {
    let mut workbook = Workbook::new();
    let sheet = workbook.add_worksheet();
    sheet
        .set_name(&spec.sheet)
        .map_err(|e| Error::Runtime(format!("设置 sheet 名失败: {e}")))?;

    // 表头
    for (col, c) in spec.columns.iter().enumerate() {
        sheet
            .write_string(0, col as u16, &c.header)
            .map_err(|e| Error::Runtime(format!("写表头失败: {e}")))?;
    }

    // 数据行
    for (row, r) in data.rows.iter().enumerate() {
        for (col, c) in spec.columns.iter().enumerate() {
            let value = r.get(&c.field).cloned().unwrap_or(Value::Empty);
            write_cell(&mut *sheet, (row + 1) as u32, col as u16, &value)?;
        }
    }

    workbook
        .save(path)
        .map_err(|e| Error::Runtime(format!("保存 {path:?} 失败: {e}")))?;
    Ok(())
}

fn write_cell(sheet: &mut Worksheet, row: u32, col: u16, v: &Value) -> Result<()> {
    match v {
        Value::Num(n) => sheet
            .write(row, col, *n)
            .map_err(|e| Error::Runtime(format!("写数值失败: {e}"))),
        Value::Str(s) => sheet
            .write(row, col, s)
            .map_err(|e| Error::Runtime(format!("写字符串失败: {e}"))),
        Value::Bool(b) => sheet
            .write(row, col, *b)
            .map_err(|e| Error::Runtime(format!("写布尔失败: {e}"))),
        Value::Empty => sheet
            .write_string(row, col, "")
            .map_err(|e| Error::Runtime(format!("写空失败: {e}"))),
    }
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calc::Row;
    use crate::scheme::NewColumn;
    use std::collections::HashMap;

    fn sample_data() -> DerivedTable {
        let mut r1 = Row::new();
        r1.insert("姓名".into(), Value::Str("甲".into()));
        r1.insert("节数".into(), Value::Num(3.0));
        r1.insert("费用".into(), Value::Num(21.0));
        let mut r2 = Row::new();
        r2.insert("姓名".into(), Value::Str("乙".into()));
        r2.insert("节数".into(), Value::Num(2.0));
        r2.insert("费用".into(), Value::Num(14.0));
        DerivedTable {
            columns: vec!["姓名".into(), "节数".into(), "费用".into()],
            rows: vec![r1, r2],
        }
    }

    fn output_spec() -> NewFileOutput {
        NewFileOutput {
            file: "{monthDir}/补课时.xlsx".into(),
            from: "agg".into(),
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
        }
    }

    #[test]
    fn writes_and_reads_back() {
        let path = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../target/m1-newfile.xlsx"
        ));
        let spec = output_spec();
        write_new_file(path, &spec, &sample_data()).expect("写文件失败");

        let mut wb = calamine::open_workbook_auto(path).expect("回读失败");
        use calamine::Reader;
        let range = wb.worksheet_range("替上课补课时").expect("读 sheet 失败");
        let (rows, _) = range.get_size();
        assert_eq!(rows, 3, "表头 + 2 行");

        let mut by_name: HashMap<String, (f64, f64)> = HashMap::new();
        for r in 1..rows {
            let name = match range.get_value((r as u32, 0)) {
                Some(calamine::Data::String(s)) => s.clone(),
                _ => continue,
            };
            let jie = match range.get_value((r as u32, 1)) {
                Some(calamine::Data::Int(i)) => *i as f64,
                Some(calamine::Data::Float(f)) => *f,
                _ => continue,
            };
            let fee = match range.get_value((r as u32, 2)) {
                Some(calamine::Data::Int(i)) => *i as f64,
                Some(calamine::Data::Float(f)) => *f,
                _ => continue,
            };
            by_name.insert(name, (jie, fee));
        }
        assert_eq!(by_name.get("甲"), Some(&(3.0, 21.0)));
        assert_eq!(by_name.get("乙"), Some(&(2.0, 14.0)));
    }
}
