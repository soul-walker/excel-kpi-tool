//! 临时探针：批量打印每个 xlsx 的 sheet 与解析出的单元格数（诊断用）。
//! 用法: cargo run -p kpi-engine --example probe_read -- <file...>

use umya_spreadsheet::reader;

fn main() {
    for input in std::env::args().skip(1) {
        println!("==== {input}");
        match reader::xlsx::read(&input) {
            Ok(book) => {
                for ws in book.sheet_collection() {
                    println!("  sheet={} cells={}", ws.name(), ws.cells().len());
                }
            }
            Err(e) => println!("  ERROR: {e}"),
        }
    }
}
