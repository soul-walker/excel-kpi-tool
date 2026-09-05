//! engine 桥接（纯函数，供 Bevy observer 调用；可单测）。
//! 只做 字符串入 → 结果 JSON 字符串 出，不接触 Godot API。

use kpi_engine::runner::{RunOptions, run_scheme_outputs};
use kpi_engine::scheme::VersionedScheme;

/// 校验方案，返回 `{ ok, name?, issues }` 或 `{ ok:false, error }` 的 JSON 字符串。
pub fn validate_scheme_json(path: &str) -> String {
    let raw = match std::fs::read_to_string(path) {
        Ok(r) => r,
        Err(e) => return err_json(&format!("读取方案失败: {e}")),
    };
    let versioned = match VersionedScheme::from_json_str(&raw) {
        Ok(v) => v,
        Err(e) => return err_json(&format!("方案解析失败: {e}")),
    };
    let issues: Vec<serde_json::Value> = versioned
        .scheme
        .validate()
        .iter()
        .map(|i| {
            serde_json::json!({
                "severity": if i.severity == kpi_engine::scheme::Severity::Error { "error" } else { "warning" },
                "location": i.location,
                "message": i.message,
            })
        })
        .collect();
    serde_json::json!({
        "ok": issues.iter().all(|i| i["severity"] != "error"),
        "name": versioned.scheme.name,
        "issues": issues,
    })
    .to_string()
}

/// 运行方案（dry-run 或实际写回），返回执行结果 JSON。
pub fn run_scheme_json(scheme_path: &str, month_dir: &str, dry_run: bool) -> String {
    let raw = match std::fs::read_to_string(scheme_path) {
        Ok(r) => r,
        Err(e) => return err_json(&format!("读取方案失败: {e}")),
    };
    let versioned = match VersionedScheme::from_json_str(&raw) {
        Ok(v) => v,
        Err(e) => return err_json(&format!("方案解析失败: {e}")),
    };
    match run_scheme_outputs(
        &versioned.scheme,
        std::path::Path::new(month_dir),
        RunOptions { dry_run },
    ) {
        Ok(summary) => {
            let outcomes: Vec<serde_json::Value> = summary
                .outcomes
                .iter()
                .map(|o| {
                    serde_json::json!({
                        "id": o.output_id,
                        "kind": o.kind,
                        "target": o.target.to_string_lossy(),
                        "wrote": o.wrote,
                        "detail": o.detail,
                    })
                })
                .collect();
            serde_json::json!({ "ok": true, "dry_run": dry_run, "outcomes": outcomes }).to_string()
        }
        Err(e) => err_json(&format!("运行失败: {e}")),
    }
}

/// 把 JSON 结果渲染成便于 GUI/日志展示的多行文本（"❌ error" 或 pretty JSON）。
pub fn render_result(raw: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(v) => {
            if let Some(false) = v.get("ok").and_then(|o| o.as_bool()) {
                let err = v
                    .get("error")
                    .and_then(|e| e.as_str())
                    .unwrap_or("未知错误");
                format!("❌ {err}")
            } else {
                serde_json::to_string_pretty(&v).unwrap_or_else(|_| raw.to_string())
            }
        }
        Err(_) => raw.to_string(),
    }
}

fn err_json(msg: &str) -> String {
    serde_json::json!({ "ok": false, "error": msg }).to_string()
}
