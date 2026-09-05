//! M0 原型验证：XML 最小 patch 回写（calamine 定位 + zip/XML patch）。
//!
//! 用法（仓库根）：
//!   cargo run -p kpi-engine --example spike_xmlpatch -- \
//!       testdata/2023-09-golden/kpi/绩效.xlsx target/patch-绩效.xlsx
//!
//! 做法：用 calamine 定位"表头行(按内容)"与 key 行 → 只改写两列的若干单元格
//!       → 重新打开校验写入值与未动单元格。

use std::path::Path;

use calamine::{Data, Reader, open_workbook_auto};
use kpi_engine::{SheetEdit, cell_ref, locate_sheet, patch_workbook};

const SHEETS: [&str; 2] = ["非校聘", "校聘"];
const EXPECTED: [&str; 3] = ["姓名", "课时S", "实际发放"];

fn read_cell(path: &Path, sheet: &str, row: u32, col: u32) -> Option<Data> {
    let mut wb = open_workbook_auto(path).expect("打开失败");
    let range = wb.worksheet_range(sheet).expect("读 sheet 失败");
    range.get_value((row - 1, col - 1)).cloned()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("用法: spike_xmlpatch <输入模板.xlsx> <输出副本.xlsx>");
        std::process::exit(2);
    }
    let input = Path::new(&args[1]);
    let output = Path::new(&args[2]);

    let mut edits: Vec<SheetEdit> = Vec::new();
    let mut expects: Vec<(String, u32, u32, Data)> = Vec::new();

    for sheet in SHEETS {
        let loc = locate_sheet(input, sheet, &EXPECTED, "姓名")?;
        println!(
            "[locate] {sheet}: 表头行={} 列={:?} 数据人数={}",
            loc.header_row,
            loc.col_of,
            loc.key_rows.len()
        );
        assert_eq!(
            loc.header_row, 2,
            "{sheet} 表头应在第 2 行（标题在第 1 行）"
        );

        let (key, row) = &loc.key_rows[0];
        let col_ksf = loc.col_of["课时S"];
        let col_sjff = loc.col_of["实际发放"];

        let before = read_cell(input, sheet, *row, col_ksf);
        println!("[target] {sheet}: key={key} row={row} 课时S 旧值={before:?}");

        let v_ksf = if sheet == "非校聘" {
            "111.5"
        } else {
            "122.5"
        }
        .to_string();
        let v_sjff = "222".to_string();
        edits.push(SheetEdit {
            sheet: sheet.to_string(),
            cells: vec![
                (cell_ref(*row, col_ksf), v_ksf.clone()),
                (cell_ref(*row, col_sjff), v_sjff.clone()),
            ],
        });
        expects.push((
            sheet.to_string(),
            *row,
            col_ksf,
            Data::Float(v_ksf.parse()?),
        ));
        expects.push((sheet.to_string(), *row, col_sjff, Data::Float(222.0)));
    }

    let changed = patch_workbook(input, output, &edits)?;
    println!("[patch ] 改写条目: {changed:?}");

    // 校验：写入值正确；未动单元格（姓名列）保持一致。
    for (sheet, row, col, want) in &expects {
        let got = read_cell(output, sheet, *row, *col).expect("单元格应存在");
        assert_eq!(
            got, *want,
            "{sheet} ({row},{col}) 写入不一致: got={got:?} want={want:?}"
        );
        println!("[verify] {sheet} ({row},{col}) = {got:?} OK");
    }
    for sheet in SHEETS {
        let loc = locate_sheet(input, sheet, &EXPECTED, "姓名")?;
        let (key, row) = &loc.key_rows[0];
        let col_name = loc.col_of["姓名"];
        let before = read_cell(input, sheet, *row, col_name);
        let after = read_cell(output, sheet, *row, col_name);
        assert_eq!(before, after, "{sheet} 姓名列被意外改动");
        println!("[keep  ] {sheet} 姓名={key:?} 未被改动 OK");
    }

    println!("XML 最小 patch 原型 OK。可对输出做 zip/逐部件 diff 复核。");
    Ok(())
}
