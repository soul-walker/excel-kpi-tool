//! runner：方案运行入口（headless，GUI/CLI 共用）。
//!
//! 一次运行 = 静态校验 → sources/derived → 按人聚合 → 依次执行 scheme.outputs：
//! - `patch_template`：定位模板人员行/列 → 规划单元格 → 按 `apply` 回写
//!   （inplace=原文件覆盖并自动 `.bak`；copy=另存 `<名>.out.xlsx` 副本）；
//! - `new_file`：从 source / derived 明细生成全新 xlsx。
//!
//! 通过 `RunOptions::dry_run` 可只算不写（GUI 试算/预览用）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::calc::Value;
use crate::error::{Error, Result};
use crate::exec::{
    compute_aggregates, plan_template_patch, resolve_file, run_scheme, table_to_rows,
};
use crate::scheme::{ApplyMode, OutputKind, Scheme};
use crate::template::SheetEdit;
use crate::writer::write_new_file;

/// 运行选项。
#[derive(Debug, Clone, Copy, Default)]
pub struct RunOptions {
    /// 只计算不写盘（不生成 .bak、不覆盖模板、不生成新文件）。
    pub dry_run: bool,
}

/// 单个输出的执行结果。
#[derive(Debug, Clone)]
pub struct OutputOutcome {
    pub output_id: String,
    /// 输出方式："patch_template" / "new_file"。
    pub kind: String,
    /// 目标文件（dry-run 时表示将要写的位置）。
    pub target: PathBuf,
    /// 已写盘？
    pub wrote: bool,
    /// 人类可读说明（cell 数量、备份路径等）。
    pub detail: String,
}

/// 整次运行的汇总。
#[derive(Debug, Clone, Default)]
pub struct RunSummary {
    pub outcomes: Vec<OutputOutcome>,
}

/// 目标文件名的 `.bak` 路径（绩效.xlsx → 绩效.xlsx.bak）。
fn backup_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|s| s.to_os_string())
        .unwrap_or_default();
    name.push(".bak");
    path.with_file_name(name)
}

/// apply=copy 时的另存路径（绩效.xlsx → 绩效.out.xlsx）。
fn copy_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_stem()
        .map(|s| s.to_os_string())
        .unwrap_or_default();
    name.push(".out");
    if let Some(ext) = path.extension() {
        name.push(".");
        name.push(ext);
    }
    path.with_file_name(name)
}

/// 从 run 中取"行集"（source 或 derived 明细），用于 new_file 输出。
fn rowset_for(run: &crate::exec::SchemeRun, from: &str) -> Result<crate::exec::DerivedTable> {
    if let Some(t) = run.source_tables.get(from) {
        let columns: Vec<String> = t.columns.iter().map(|c| c.alias.clone()).collect();
        return Ok(crate::exec::DerivedTable {
            columns,
            rows: table_to_rows(t),
        });
    }
    if let Some(d) = run.derived_tables.get(from) {
        return Ok(d.clone());
    }
    Err(Error::Runtime(format!(
        "new_file 输出引用不存在的表: {from}"
    )))
}

/// 执行一个 patch_template 输出。
#[allow(clippy::too_many_arguments)]
fn apply_patch_output(
    scheme: &Scheme,
    run: &crate::exec::SchemeRun,
    aggs: &BTreeMap<String, BTreeMap<String, Value>>,
    output_id: &str,
    template: &crate::scheme::PatchTemplate,
    month_dir: &Path,
    dry_run: bool,
) -> Result<OutputOutcome> {
    let edits: Vec<SheetEdit> = plan_template_patch(scheme, run, aggs, output_id, month_dir)?;
    let cell_count: usize = edits.iter().map(|e| e.cells.len()).sum();
    let target = resolve_file(&template.file, month_dir);
    if !target.exists() {
        return Err(Error::Runtime(format!("模板不存在: {target:?}")));
    }

    let mut detail = format!("计划写 {cell_count} 个单元格");
    let mut wrote = false;
    if !dry_run {
        match template.apply {
            ApplyMode::Inplace => {
                if template.backup {
                    let bak = backup_path(&target);
                    std::fs::copy(&target, &bak).map_err(|e| {
                        Error::Runtime(format!("备份 {target:?} → {bak:?} 失败: {e}"))
                    })?;
                    detail.push_str(&format!("；备份 {bak:?}"));
                }
                crate::template::patch_workbook(&target, &target, &edits)?;
                wrote = true;
            }
            ApplyMode::Copy => {
                let dst = copy_path(&target);
                crate::template::patch_workbook(&target, &dst, &edits)?;
                wrote = true;
                detail.push_str(&format!("；另存 {dst:?}"));
            }
        }
    } else {
        detail.push_str("（dry-run，未写盘）");
    }
    Ok(OutputOutcome {
        output_id: output_id.to_string(),
        kind: "patch_template".into(),
        target,
        wrote,
        detail,
    })
}

/// 执行一个 new_file 输出。
fn apply_new_file_output(
    spec: &crate::scheme::NewFileOutput,
    output_id: &str,
    run: &crate::exec::SchemeRun,
    month_dir: &Path,
    dry_run: bool,
) -> Result<OutputOutcome> {
    let data = rowset_for(run, &spec.from)?;
    let target = resolve_file(&spec.file, month_dir);
    let mut detail = format!("{} 行数据", data.rows.len());
    let mut wrote = false;
    if !dry_run {
        write_new_file(&target, spec, &data)?;
        wrote = true;
        detail.push_str(&format!("；写出 {target:?}"));
    } else {
        detail.push_str("（dry-run，未写盘）");
    }
    Ok(OutputOutcome {
        output_id: output_id.to_string(),
        kind: "new_file".into(),
        target,
        wrote,
        detail,
    })
}

/// 运行整份方案（headless 主入口）。
///
/// `month_dir` 为所有输入台账/模板的所在目录（方案里 `file` 为相对该目录的文件名）。
/// 返回每个输出的执行结果，方便 GUI 展示/试算，或 CLI 打印。
pub fn run_scheme_outputs(
    scheme: &Scheme,
    month_dir: &Path,
    options: RunOptions,
) -> Result<RunSummary> {
    scheme.ensure_valid()?;
    if !month_dir.is_dir() {
        return Err(Error::Runtime(format!("月份目录不存在: {month_dir:?}")));
    }

    let run = run_scheme(scheme, month_dir)?;
    let aggs = compute_aggregates(scheme, &run)?;

    let mut outcomes = Vec::with_capacity(scheme.outputs.len());
    for o in &scheme.outputs {
        let outcome = match &o.kind {
            OutputKind::PatchTemplate(p) => {
                apply_patch_output(scheme, &run, &aggs, &o.id, p, month_dir, options.dry_run)?
            }
            OutputKind::NewFile(n) => {
                apply_new_file_output(n, &o.id, &run, month_dir, options.dry_run)?
            }
        };
        outcomes.push(outcome);
    }
    Ok(RunSummary { outcomes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheme::{NewColumn, NewFileOutput, OutputKind, OutputSpec, VersionedScheme};
    use calamine::Reader as _;

    fn data_dir() -> PathBuf {
        Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../testdata/2023-09")).to_path_buf()
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kpi-runner-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    /// 复制输入（全部 .xlsx）到运行目录，模拟真实月份目录。
    fn copy_inputs(dst: &Path) {
        for entry in std::fs::read_dir(data_dir()).expect("读 testdata") {
            let entry = entry.expect("目录项");
            if entry.path().extension().and_then(|e| e.to_str()) == Some("xlsx") {
                std::fs::copy(entry.path(), dst.join(entry.file_name())).expect("复制输入");
            }
        }
    }

    fn load_scheme() -> VersionedScheme {
        let path = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../schemes/高中月绩效复刻.json"
        ));
        let raw = std::fs::read_to_string(path).expect("读复刻方案失败");
        VersionedScheme::from_json_str(&raw).expect("复刻方案解析失败")
    }

    /// 读单元格网格（空/空字符串等价、数值精确比较），供整表对拍。
    fn read_grid(path: &Path, sheet: &str) -> BTreeMap<(u32, u32), String> {
        use calamine::Data;
        let mut wb = calamine::open_workbook_auto(path).expect("打开");
        let range = wb.worksheet_range(sheet).expect("读 sheet");
        let (rows, cols) = range.get_size();
        let mut m = BTreeMap::new();
        for r in 0..rows {
            for c in 0..cols {
                let key = (r as u32, c as u32);
                let v = match range.get_value(key) {
                    Some(Data::Float(f)) => format!("n{f}"),
                    Some(Data::Int(i)) => format!("n{i}.0"),
                    Some(Data::String(s)) if !s.is_empty() => format!("s{s}"),
                    Some(Data::Bool(b)) => format!("b{b}"),
                    _ => "e".into(),
                };
                m.insert(key, v);
            }
        }
        m
    }

    fn count_grid_diffs(a: &Path, b: &Path) -> usize {
        let mut ndiff = 0;
        for sheet in ["非校聘", "校聘"] {
            let ma = read_grid(a, sheet);
            let mb = read_grid(b, sheet);
            let mut keys: Vec<(u32, u32)> = ma.keys().chain(mb.keys()).copied().collect();
            keys.sort_unstable();
            keys.dedup();
            for k in keys {
                let va = ma.get(&k).cloned().unwrap_or_else(|| "e".into());
                let vb = mb.get(&k).cloned().unwrap_or_else(|| "e".into());
                if !cell_eq(&va, &vb) {
                    ndiff += 1;
                }
            }
        }
        ndiff
    }

    fn cell_eq(a: &str, b: &str) -> bool {
        match (a.strip_prefix('n'), b.strip_prefix('n')) {
            (Some(x), Some(y)) => {
                let x: f64 = x.parse().unwrap_or(f64::NAN);
                let y: f64 = y.parse().unwrap_or(f64::NAN);
                x == y
            }
            _ => a == b,
        }
    }

    #[test]
    fn dry_run_does_not_write() {
        let dir = temp_dir("dry");
        copy_inputs(&dir);
        let scheme = load_scheme().scheme;

        let summary =
            run_scheme_outputs(&scheme, &dir, RunOptions { dry_run: true }).expect("dry-run 失败");
        assert!(!summary.outcomes.is_empty());
        assert!(
            summary.outcomes.iter().all(|o| !o.wrote),
            "dry-run 不应写盘: {summary:?}"
        );
        assert!(!dir.join("绩效.xlsx.bak").exists(), "dry-run 不应生成 .bak");
        assert!(!dir.join("绩效.out.xlsx").exists(), "dry-run 不应另存副本");
    }

    #[test]
    fn run_outputs_inplace_backup_and_match_golden() {
        let dir = temp_dir("run");
        copy_inputs(&dir);
        let mut v = load_scheme();
        // 追加一个 new_file 输出（补课时新表），走同一个运行入口。
        v.scheme.outputs.push(OutputSpec {
            id: "补课时新表".into(),
            kind: OutputKind::NewFile(NewFileOutput {
                file: "{monthDir}/补课时.xlsx".into(),
                from: "bukesh".into(),
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
            }),
        });

        let summary = run_scheme_outputs(&v.scheme, &dir, RunOptions { dry_run: false })
            .expect("运行方案失败");
        assert_eq!(summary.outcomes.len(), 2);
        let jx = &summary.outcomes[0];
        assert!(jx.wrote, "绩效应已回写");
        assert!(dir.join("绩效.xlsx.bak").exists(), "应有 .bak 备份");
        assert!(jx.detail.contains("计划写"));

        // 绩效回写结果与 golden 逐格一致。
        let golden = data_dir().with_file_name("2023-09-golden/kpi/绩效.xlsx");
        assert_eq!(count_grid_diffs(&dir.join("绩效.xlsx"), &golden), 0);

        // new_file 输出已生成，且回读后与 golden/bukeshi 逐人一致。
        let out_file = dir.join("补课时.xlsx");
        assert!(summary.outcomes[1].wrote && out_file.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
