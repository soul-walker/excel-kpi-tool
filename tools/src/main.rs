//! 研发工具（xtask 式）统一入口：环境配置与后续开发 / 测试 / 发布命令。
//!
//! 用法（在 workspace 根执行）：
//!   cargo run -p tools -- setup    配置开发环境（README「环境安装与配置」）
//!
//! TODO: 后续在此扩展 build / test / bench / release 等子命令。

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

/// workspace 根 = tools crate 的上一级目录（编译期确定，与运行目录无关）。
fn workspace_root() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("tools 应位于 workspace 根的子目录")
            .to_path_buf()
    })
}

/// setup 所需的二进制工具及其安装命令。
const REQUIRED_TOOLS: &[(&str, &str)] = &[
    ("prek", "cargo install prek"),
    ("typos", "cargo install typos-cli"),
];

#[derive(Debug, Parser)]
#[command(
    name = "tools",
    version,
    about = "研发工具（xtask 式）：开发环境配置与构建/测试/发布"
)]
struct Cli {
    #[command(subcommand)]
    task: Task,
}

#[derive(Debug, Subcommand)]
enum Task {
    /// 检查 prek / typos-cli，初始化 git 仓库，并安装 prek 提交钩子
    Setup,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.task {
        Task::Setup => setup(),
    }
}

/// `setup`：检查环境、初始化 git 并安装 prek 提交钩子。
fn setup() -> Result<()> {
    let root = workspace_root();

    // 1. 检查二进制工具是否可用。
    let mut missing_prek = false;
    for (bin, install) in REQUIRED_TOOLS {
        if bin_exists(bin) {
            println!("✓ 已安装: {bin}");
        } else {
            println!("✗ 未找到: {bin}（安装：{install}）");
            missing_prek |= *bin == "prek";
        }
    }

    // 2. 确保项目根是独立 git 仓库（钩子写入本项目的 .git/hooks）。
    if !bin_exists("git") {
        bail!("未找到 git，请先安装：https://git-scm.com");
    }
    if !root.join(".git").exists() {
        println!("未检测到 .git，执行 `git init -b main` ...");
        run_in(root, "git", &["init", "-b", "main"])?;
    }

    if missing_prek {
        bail!("缺少 prek；安装完成后重新运行 `cargo run -p tools -- setup`");
    }

    // 3. 安装 prek 提交钩子。
    println!("安装 prek 提交钩子 ...");
    run_in(root, "prek", &["install"])?;
    println!();
    println!("开发环境就绪。提交时将自动执行：");
    println!("  cargo fmt -- --check / cargo check --all / cargo clippy -D warnings / typos");
    println!("钩子与拼写配置见仓库根 prek.toml 与 _typos.toml。");
    Ok(())
}

fn bin_exists(bin: &str) -> bool {
    Command::new(bin).arg("--version").output().is_ok()
}

fn run_in(dir: &Path, program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(program)
        .args(args)
        .current_dir(dir)
        .status()
        .context(format!("无法执行 {program}"))?;
    if status.success() {
        Ok(())
    } else {
        bail!("{program} 执行失败（退出码: {:?}）", status.code());
    }
}
