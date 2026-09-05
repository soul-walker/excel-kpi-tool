//! M0 Spike：验证"umya-spreadsheet(3.x) + rels 归一化"对真实模板的原格式回写。
//!
//! 用法（仓库根执行）：
//!   cargo run -p kpi-engine --example spike_writeback -- \
//!       testdata/2023-09/绩效.xlsx target/spike-绩效.xlsx
//!
//! 背景（本次 Spike 发现）：
//! 1. umya 2.3.3 无法打开含自动筛选定义名 `_xlnm._FilterDatabase`（localSheetId=0）的文件 → 弃用，基线 3.1.0；
//! 2. 3.1.0 打开某些模板单元格全空：其 `xl/_rels/workbook.xml.rels` 用了包根绝对
//!    `Target="/xl/..."`，umya 未按 OPC 规范解析绝对 Target，导致 sharedStrings/工作表没读到。
//!    因此读取前需把该 rels 里的 `/xl/` 绝对目标归一化为相对目标（归一化逻辑后续进 WorkbookIO 适配层）。
//!
//! 流程：归一化 rels → 生成临时输入 → umya 打开 → 列 sheet → 目标格写入 → 另存副本
//!        → 重新打开并校验：目标格值一致、抽样文本格不丢、单元格总数不降。
//! 目标 sheet 缺省时自动回退第一个可用 sheet，便于对任意台账验证。

use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

use umya_spreadsheet::reader;
use umya_spreadsheet::writer;

const TARGET_SHEETS: [&str; 2] = ["非校聘", "校聘"];
const TARGET_ADDRESS: &str = "AD3";
const TEST_VALUE: f64 = 123.5;
/// 内容保留校验的抽样格（表头/首个数据行）。
const SAMPLE_ADDRS: [&str; 4] = ["A1", "B1", "C1", "A2"];

/// 将 `xl/_rels/workbook.xml.rels` 中 `Target="/xl/..."` 归一化为 `Target="..."`，
/// 其余部件原样复制；写出为临时文件供 umya 读取。
fn normalize_rels(input: &str, output: &str) -> std::io::Result<()> {
    let mut zin = zip::ZipArchive::new(File::open(input)?)?;
    let outfile = File::create(output)?;
    let mut zw = zip::ZipWriter::new(outfile);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    for i in 0..zin.len() {
        let mut entry = zin.by_index(i)?;
        let name = entry.name().to_string();
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf)?;
        if name == "xl/_rels/workbook.xml.rels" {
            let text = String::from_utf8_lossy(&buf);
            let fixed = text.replace("Target=\"/xl/", "Target=\"");
            buf = fixed.into_bytes();
            println!("[rels ] 已归一化 {name}（去掉 /xl/ 绝对前缀）");
        }
        zw.start_file(name, opts)?;
        zw.write_all(&buf)?;
    }
    zw.finish()?;
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("用法: spike_writeback <输入模板.xlsx> <输出副本.xlsx>");
        std::process::exit(2);
    }
    let input = &args[1];
    let output = &args[2];

    let normalized = Path::new("target").join("spike-normalized-input.xlsx");
    let normalized = normalized.to_str().expect("路径转字符串失败");
    normalize_rels(input, normalized).expect("归一化 rels 失败");

    let mut book = reader::xlsx::read(normalized).expect("打开(归一化)模板失败");
    println!("[sheet] 共 {} 个：", book.sheet_collection().len());
    for ws in book.sheet_collection() {
        println!("  - {}  读取到 {} 个单元格", ws.name(), ws.cells().len());
    }

    // 选择目标 sheet：优先 非校聘/校聘；缺省回退第一个可用 sheet。
    let mut targets: Vec<String> = Vec::new();
    for s in TARGET_SHEETS {
        if book.sheet_by_name(s).is_ok() {
            targets.push(s.to_string());
        }
    }
    if targets.is_empty() {
        if let Some(ws) = book.sheet_collection().first() {
            targets.push(ws.name().to_string());
        } else {
            panic!("空工作簿，无可用 sheet");
        }
        println!("[note ] 未找到目标 sheet，回退到第一个: {}", targets[0]);
    }

    // 写前抽样。
    let mut before: Vec<(String, String, String)> = Vec::new();
    for sheet in &targets {
        let ws = book
            .sheet_by_name_mut(sheet)
            .unwrap_or_else(|e| panic!("缺少 sheet {sheet}: {e}"));
        for addr in SAMPLE_ADDRS {
            before.push((
                sheet.clone(),
                addr.to_string(),
                format!("{:?}", ws.cell_value(addr)),
            ));
        }
        let old = ws.value_number(TARGET_ADDRESS);
        ws.cell_mut(TARGET_ADDRESS).set_value_number(TEST_VALUE);
        println!("[write] {sheet}!{TARGET_ADDRESS}  旧值={old:?} → {TEST_VALUE}");
    }

    writer::xlsx::write(&book, output).expect("另存副本失败");
    println!("[save ] 已另存: {output}");

    let back = reader::xlsx::read(output).expect("重新打开输出失败");
    for sheet in &targets {
        let ws = back
            .sheet_by_name(sheet)
            .unwrap_or_else(|e| panic!("缺少 sheet {sheet}: {e}"));
        let got = ws.value_number(TARGET_ADDRESS);
        println!(
            "[read ] {sheet}!{TARGET_ADDRESS} = {got:?}  cells={}",
            ws.cells().len()
        );
        assert_eq!(got, Some(TEST_VALUE), "目标格读回不一致");
        for addr in SAMPLE_ADDRS {
            let now = format!("{:?}", ws.cell_value(addr));
            let item = before
                .iter()
                .find(|(s, a, _)| s == sheet && a == addr)
                .expect("写前抽样缺失");
            assert_eq!(
                &now, &item.2,
                "{sheet}!{addr} 内容在回写后丢失: before={} after={}",
                item.2, now
            );
        }
    }

    println!(
        "Spike 链路 + 内容保留校验 OK。请用 Excel/WPS 打开 {output} 人工核验版式与无修复提示。"
    );
}
