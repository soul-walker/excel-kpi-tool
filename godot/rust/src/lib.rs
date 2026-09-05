//! kpi_tool：Godot 4 客户端（方案编辑器 + 运行台）。
//!
//! 分层（按 docs/tech.md）：
//! - Godot 场景 = 表现/布局（`godot/main.tscn` 等）；GDScript 只做薄胶水；
//! - Rust 侧：`KpiBridge`（自定义 Node）把 kpi-engine 能力暴露给 UI；
//!   Bevy ECS（`#[bevy_app]`）挂载运行状态机等（后续扩充）。
//!
//! 所有业务逻辑仍住在 `kpi-engine`（headless），本 crate 只做桥接。

use bevy::prelude::*;
use godot::classes::Node;
use godot::prelude::*;
use godot_bevy::prelude::*;

// ---------- Bevy ECS 挂载点（运行状态机骨架，后续扩充） ----------

#[bevy_app]
fn build_app(app: &mut App) {
    app.add_plugins(GodotDefaultPlugins);
    // TODO(M2): 方案会话/运行状态机 Resource + System（加载、校验、dry-run、写回）。
}

// ---------- KpiBridge：向 GDScript/UI 暴露引擎操作 ----------

#[derive(GodotClass)]
#[class(base=Node)]
pub struct KpiBridge {
    base: Base<Node>,
}

#[godot_api]
impl INode for KpiBridge {
    fn init(base: Base<Node>) -> Self {
        Self { base }
    }
}

#[godot_api]
impl KpiBridge {
    /// 连通性测试。
    #[func]
    fn ping(&self) -> GString {
        "pong".into()
    }

    /// 校验方案：`{ "ok": bool, "name": string, "issues": [ {severity, location, message} ] }`。
    #[func]
    fn validate_scheme(&self, path: GString) -> GString {
        let path = path.to_string();
        let raw = match std::fs::read_to_string(&path) {
            Ok(r) => r,
            Err(e) => return err_json(&format!("读取方案失败: {e}")),
        };
        let versioned = match kpi_engine::scheme::VersionedScheme::from_json_str(&raw) {
            Ok(v) => v,
            Err(e) => return err_json(&format!("方案解析失败: {e}")),
        };
        let name = versioned.scheme.name.clone();
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
        let ok = issues.is_empty();
        let s = serde_json::json!({ "ok": ok, "name": name, "issues": issues }).to_string();
        GString::from(s.as_str())
    }

    /// 运行方案（dry-run 或实际写回），返回执行结果 JSON 数组。
    #[func]
    fn run_scheme(&self, scheme_path: GString, month_dir: GString, dry_run: bool) -> GString {
        let scheme_path = scheme_path.to_string();
        let month_dir = month_dir.to_string();
        let raw = match std::fs::read_to_string(&scheme_path) {
            Ok(r) => r,
            Err(e) => return err_json(&format!("读取方案失败: {e}")),
        };
        let versioned = match kpi_engine::scheme::VersionedScheme::from_json_str(&raw) {
            Ok(v) => v,
            Err(e) => return err_json(&format!("方案解析失败: {e}")),
        };
        match kpi_engine::runner::run_scheme_outputs(
            &versioned.scheme,
            std::path::Path::new(&month_dir),
            kpi_engine::runner::RunOptions { dry_run },
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
                let s = serde_json::json!({ "ok": true, "dry_run": dry_run, "outcomes": outcomes })
                    .to_string();
                GString::from(s.as_str())
            }
            Err(e) => err_json(&format!("运行失败: {e}")),
        }
    }
}

fn err_json(msg: &str) -> GString {
    let s = serde_json::json!({ "ok": false, "error": msg }).to_string();
    GString::from(s.as_str())
}
