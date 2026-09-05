//! 事件处理：读输入 → 调 kpi-engine（bridge）→ 写日志区并 tracing 输出。

use bevy::ecs::observer::On;
use bevy::prelude::Res;
use godot::classes::{LineEdit, ProjectSettings, RichTextLabel};
use godot::obj::Singleton;
use godot_bevy::prelude::GodotAccess;
use tracing::info;

use crate::bridge;

use super::events::{RunClicked, ValidateClicked};
use super::view::{UiNodes, UiRefs};

/// 把一行追加进日志区（RichTextLabel）。
fn log_to_ui(nodes: &UiNodes, godot: &mut GodotAccess, line: &str) {
    let mut log = godot.get::<RichTextLabel>(nodes.log);
    log.append_text(&format!("{line}\n"));
}

/// 把引擎返回的 JSON 结果同时输出到 tracing 与日志区。
fn append_result(nodes: &UiNodes, godot: &mut GodotAccess, json: &str) {
    let pretty = bridge::render_result(json);
    info!("{pretty}");
    log_to_ui(nodes, godot, &pretty);
}

/// 把 Godot 虚拟路径（res://…）换成文件系统路径，其它路径原样返回。
fn global_path(path: &str) -> String {
    if path.starts_with("res://") {
        ProjectSettings::singleton()
            .globalize_path(path)
            .to_string()
    } else {
        path.to_string()
    }
}

pub fn on_validate_clicked(
    _trigger: On<ValidateClicked>,
    refs: Res<UiRefs>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = &refs.nodes else {
        return;
    };
    let raw = godot
        .get::<LineEdit>(nodes.scheme_path)
        .get_text()
        .to_string();
    let scheme = global_path(&raw);
    log_to_ui(nodes, &mut godot, &format!("→ 校验 {scheme}"));
    append_result(nodes, &mut godot, &bridge::validate_scheme_json(&scheme));
}

pub fn on_run_clicked(trigger: On<RunClicked>, refs: Res<UiRefs>, mut godot: GodotAccess) {
    let Some(nodes) = &refs.nodes else {
        return;
    };
    let raw_scheme = godot
        .get::<LineEdit>(nodes.scheme_path)
        .get_text()
        .to_string();
    let raw_month = godot
        .get::<LineEdit>(nodes.month_dir)
        .get_text()
        .to_string();
    let scheme = global_path(&raw_scheme);
    let month = global_path(&raw_month);
    let dry = trigger.event().dry_run;
    let label = if dry {
        "Dry-run 预览"
    } else {
        "实际运行"
    };
    log_to_ui(nodes, &mut godot, &format!("→ {label}：{scheme} @ {month}"));

    let json = bridge::run_scheme_json(&scheme, &month, dry);
    append_result(nodes, &mut godot, &json);
    if !dry {
        info!("实际运行完成：请检查 {month} 下的 .bak 与输出文件");
    }
}
