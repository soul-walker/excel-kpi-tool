//! 运行台事件处理：读下拉/目录 → 调 kpi-engine（bridge）→ 写日志区并 tracing 输出。

use bevy::ecs::observer::On;
use bevy::prelude::Res;
use godot::classes::{FileDialog, Label, OptionButton, RichTextLabel};
use godot_bevy::prelude::GodotAccess;
use tracing::info;

use crate::bridge;

use super::events::{BrowseMonthClicked, MonthDirPicked, RunClicked, ValidateClicked};
use super::schemes;
use super::view::{LogKind, UiNodes, UiRefs, log_line};

/// 当前选中的方案完整路径。
fn selected_scheme_path(nodes: &UiNodes, godot: &mut GodotAccess) -> String {
    let name = godot
        .get::<OptionButton>(nodes.scheme_select)
        .get_text()
        .to_string();
    schemes::scheme_path(&name)
}

/// 当前月份目录（目录标签里存的是绝对路径）。
fn month_dir(nodes: &UiNodes, godot: &mut GodotAccess) -> String {
    godot
        .get::<Label>(nodes.month_dir_label)
        .get_text()
        .to_string()
}

/// 把一行追加进日志区（RichTextLabel，按 kind 着色）。
fn log_to_ui(nodes: &UiNodes, godot: &mut GodotAccess, kind: LogKind, line: &str) {
    let mut log = godot.get::<RichTextLabel>(nodes.log);
    log_line(&mut log, kind, line);
}

/// 把引擎返回的 JSON 结果同时输出到 tracing 与日志区（按 ok/err 着色）。
fn append_result(nodes: &UiNodes, godot: &mut GodotAccess, json: &str) {
    let out = bridge::render_result(json);
    info!("{}", out.text);
    let kind = if out.ok { LogKind::Ok } else { LogKind::Err };
    log_line(&mut godot.get::<RichTextLabel>(nodes.log), kind, &out.text);
}

pub fn on_validate_clicked(
    _trigger: On<ValidateClicked>,
    refs: Res<UiRefs>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = &refs.nodes else {
        return;
    };
    let scheme = selected_scheme_path(nodes, &mut godot);
    log_to_ui(
        nodes,
        &mut godot,
        LogKind::Plain,
        &format!("→ 校验 {scheme}"),
    );
    append_result(nodes, &mut godot, &bridge::validate_scheme_json(&scheme));
}

pub fn on_run_clicked(trigger: On<RunClicked>, refs: Res<UiRefs>, mut godot: GodotAccess) {
    let Some(nodes) = &refs.nodes else {
        return;
    };
    let scheme = selected_scheme_path(nodes, &mut godot);
    let month = month_dir(nodes, &mut godot);
    let dry = trigger.event().dry_run;
    let label = if dry {
        "Dry-run 预览"
    } else {
        "实际运行"
    };
    log_to_ui(
        nodes,
        &mut godot,
        LogKind::Plain,
        &format!("→ {label}：{scheme} @ {month}"),
    );

    let json = bridge::run_scheme_json(&scheme, &month, dry);
    append_result(nodes, &mut godot, &json);
    if !dry {
        info!("实际运行完成：请检查 {month} 下的 .bak 与输出文件");
    }
}

/// “选择…”：弹出月份目录文件对话框。
pub fn on_browse_month(
    _trigger: On<BrowseMonthClicked>,
    refs: Res<UiRefs>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = &refs.nodes else {
        return;
    };
    let mut dialog = godot.get::<FileDialog>(nodes.month_dialog);
    dialog.popup_centered_ratio();
}

/// FileDialog 选定目录 → 更新月份目录标签。
pub fn on_month_dir_picked(trigger: On<MonthDirPicked>, refs: Res<UiRefs>, mut godot: GodotAccess) {
    let Some(nodes) = &refs.nodes else {
        return;
    };
    let dir = trigger.event().dir.clone();
    info!("月份目录已选择：{dir}");
    godot.get::<Label>(nodes.month_dir_label).set_text(&dir);
}
