//! 临时探针：用 calamine 读取文件，看能否解析出 绩效/补课时 等模板内容（诊断用）。
//! 用法: cargo run -p kpi-engine --example probe_calamine -- <file...>

use calamine::{Reader, open_workbook_auto};

fn main() {
    for path in std::env::args().skip(1) {
        println!("==== {path}");
        let mut wb = match open_workbook_auto(&path) {
            Ok(wb) => wb,
            Err(e) => {
                println!("  ERROR: {e}");
                continue;
            }
        };
        let sheets = wb.sheet_names().to_vec();
        for name in &sheets {
            match wb.worksheet_range(name) {
                Ok(range) => {
                    let (rows, cols) = range.get_size();
                    println!("  sheet={name}  rows={rows} cols={cols}");
                    for r in 0..rows.min(2) {
                        for c in 0..cols.min(6) {
                            if let Some(v) = range.get_value((r as u32, c as u32)) {
                                print!("  [{r},{c}]={v:?}");
                            }
                        }
                        println!();
                    }
                }
                Err(e) => println!("  sheet={name}  ERROR: {e}"),
            }
        }
    }
}
