//! engine 桥接（纯函数，供 Bevy observer 调用；可单测）。
//! 只做 字符串入 → 结果 JSON 字符串 出，不接触 Godot API。

use kpi_engine::runner::{RunOptions, run_scheme_outputs};
use kpi_engine::scheme::VersionedScheme;

/// 校验方案（按路径读取文件），返回 `{ ok, name?, issues }` 或 `{ ok:false, error }` 的 JSON 字符串。
pub fn validate_scheme_json(path: &str) -> String {
    let raw = match std::fs::read_to_string(path) {
        Ok(r) => r,
        Err(e) => return err_json(&format!("读取方案失败: {e}")),
    };
    validate_scheme_json_text(&raw)
}

/// 校验方案文本（编辑器用：校验内存中尚未落盘的编辑内容）。
pub fn validate_scheme_json_text(raw: &str) -> String {
    let versioned = match VersionedScheme::from_json_str(raw) {
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

/// 渲染结果：`ok` 供 GUI 决定整段颜色（绿/红），`text` 为多行展示文本。
pub struct RenderOut {
    pub ok: bool,
    pub text: String,
}

/// 把引擎返回的 JSON 结果渲染成便于日志展示的多行文本。
///
/// 兼容两类结果：校验（`ok`+`issues`）与运行（`ok`+`outcomes`），
/// 以及失败形态（`ok:false`+`error`）。
pub fn render_result(raw: &str) -> RenderOut {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else {
        return RenderOut {
            ok: false,
            text: raw.to_string(),
        };
    };
    let ok = v.get("ok").and_then(|o| o.as_bool()).unwrap_or(false);

    // 校验结果：issues 列表
    if let Some(issues) = v.get("issues").and_then(|i| i.as_array()) {
        if issues.is_empty() {
            let name = v
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();
            return RenderOut {
                ok: true,
                text: if name.is_empty() {
                    "校验通过".into()
                } else {
                    format!("校验通过：{name}")
                },
            };
        }
        let has_error = issues
            .iter()
            .any(|i| i.get("severity").and_then(|s| s.as_str()) == Some("error"));
        let lines: Vec<String> = issues
            .iter()
            .map(|i| {
                let sev = i.get("severity").and_then(|s| s.as_str()).unwrap_or("");
                let mark = if sev == "error" { "❌" } else { "⚠️" };
                let loc = i.get("location").and_then(|x| x.as_str()).unwrap_or("");
                let msg = i.get("message").and_then(|x| x.as_str()).unwrap_or("");
                format!("{mark} {loc}：{msg}")
            })
            .collect();
        return RenderOut {
            ok: !has_error,
            text: lines.join("\n"),
        };
    }

    // 运行/预览结果：outcomes 列表
    if let Some(outcomes) = v.get("outcomes").and_then(|o| o.as_array()) {
        let lines: Vec<String> = outcomes
            .iter()
            .map(|o| {
                let id = o.get("id").and_then(|x| x.as_str()).unwrap_or("?");
                let kind = o.get("kind").and_then(|x| x.as_str()).unwrap_or("?");
                let detail = o.get("detail").and_then(|x| x.as_str()).unwrap_or("");
                format!("• [{id}] ({kind}) {detail}")
            })
            .collect();
        return RenderOut {
            ok,
            text: lines.join("\n"),
        };
    }

    // 失败形态：error 字段
    if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
        return RenderOut {
            ok: false,
            text: err.to_string(),
        };
    }

    // 其余（应不出现）：兜底
    RenderOut {
        ok,
        text: serde_json::to_string_pretty(&v).unwrap_or_else(|_| raw.to_string()),
    }
}

fn err_json(msg: &str) -> String {
    serde_json::json!({ "ok": false, "error": msg }).to_string()
}

// ---------- 方案库：项目用户目录（user://schemes）的维护 ----------

/// 目标目录没有任何 .json 时，从 source 目录复制一份样例方案（首次使用播种）。
pub fn seed_schemes_if_empty(source_dir: &str, dest_dir: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(dest_dir)?;
    if list_json_names(dest_dir).is_empty()
        && let Ok(entries) = std::fs::read_dir(source_dir)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                let name = path.file_name().unwrap_or_default();
                std::fs::copy(&path, std::path::Path::new(dest_dir).join(name))?;
            }
        }
    }
    Ok(())
}

/// 列出目录下所有 `.json` 文件名（排序）。
pub fn list_json_names(dir: &str) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("json"))
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    names
}
