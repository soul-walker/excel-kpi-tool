//! 方案运行 CLI（headless，GUI 前的实际运行手段；GUI 直接调用同一 `runner`）。
//!
//! 用法:
//!   cargo run -p kpi-engine --example run_scheme -- <方案.json> <月份目录> [--dry-run]
//!
//! 示例:
//!   cargo run -p kpi-engine --example run_scheme -- \
//!       schemes/高中月绩效复刻.json testdata/2023-09 --dry-run
use kpi_engine::runner::{RunOptions, run_scheme_outputs};
use kpi_engine::scheme::VersionedScheme;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let scheme_path = match args.first() {
        Some(p) => p,
        None => {
            eprintln!("用法: run_scheme <方案.json> <月份目录> [--dry-run]");
            std::process::exit(2);
        }
    };
    let month_dir = match args.get(1) {
        Some(p) => p,
        None => {
            eprintln!("缺月份目录参数");
            std::process::exit(2);
        }
    };
    let dry_run = args.iter().any(|a| a == "--dry-run");

    let raw = match std::fs::read_to_string(scheme_path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("读方案失败: {e}");
            std::process::exit(2);
        }
    };
    let versioned = match VersionedScheme::from_json_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("方案解析失败: {e}");
            std::process::exit(2);
        }
    };

    let mode = if dry_run {
        "dry-run（只算不写）"
    } else {
        "实际执行"
    };
    println!("[run] {}  {} <- {month_dir}", versioned.scheme.name, mode);
    match run_scheme_outputs(
        &versioned.scheme,
        std::path::Path::new(month_dir),
        RunOptions { dry_run },
    ) {
        Ok(summary) => {
            for o in &summary.outcomes {
                println!(
                    "[out] {} [{}] {} —— {}",
                    o.output_id,
                    o.kind,
                    o.target.display(),
                    o.detail
                );
            }
            println!("[ok] {} 个输出完成", summary.outcomes.len());
        }
        Err(e) => {
            eprintln!("[err] {e}");
            std::process::exit(1);
        }
    }
}
