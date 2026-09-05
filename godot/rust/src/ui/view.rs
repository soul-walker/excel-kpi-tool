//! 类型化节点视图与句柄资源。

use bevy::prelude::{Component, Resource};
use godot::builtin::Color;
use godot::classes::RichTextLabel;
use godot::prelude::Gd;
use godot_bevy::interop::GodotNodeHandle;
use godot_bevy::prelude::NodeTreeView;

/// 日志行的语义等级（决定颜色）。
#[derive(Debug, Clone, Copy)]
pub(crate) enum LogKind {
    /// 普通信息（主题默认色）。
    Plain,
    /// 成功（绿）。
    Ok,
    /// 警告（黄）。
    Warn,
    /// 错误（红）。
    Err,
}

/// 向 RichTextLabel 追加一行；按等级着色（push_color/pop，非 bbcode 解析，
/// 因此行内出现 `[`/`]` 等字符不会被误当成标签）。
pub(crate) fn log_line(log: &mut Gd<RichTextLabel>, kind: LogKind, line: &str) {
    let color = match kind {
        LogKind::Plain => None,
        LogKind::Ok => Some(Color::from_rgb(0.45, 0.9, 0.5)),
        LogKind::Warn => Some(Color::from_rgb(0.95, 0.72, 0.25)),
        LogKind::Err => Some(Color::from_rgb(1.0, 0.48, 0.42)),
    };
    if let Some(c) = color {
        log.push_color(c);
    }
    log.append_text(&format!("{line}\n"));
    if color.is_some() {
        log.pop();
    }
}

/// 页面实体标记（供路由切页）。
#[derive(Component)]
pub struct UiHost;

/// run_console.tscn 的类型化视图。
#[derive(NodeTreeView)]
pub struct UiNodes {
    #[node("Margin/VBox/Split/ControlsBox/SchemeRow/SchemeSelect")]
    pub scheme_select: GodotNodeHandle,
    #[node("Margin/VBox/Split/ControlsBox/MonthRow/MonthDirLabel")]
    pub month_dir_label: GodotNodeHandle,
    #[node("MonthDirDialog")]
    pub month_dialog: GodotNodeHandle,
    #[node("Margin/VBox/Split/ControlsBox/ActionRow/ValidateBtn")]
    pub validate_btn: GodotNodeHandle,
    #[node("Margin/VBox/Split/ControlsBox/ActionRow/DryRunBtn")]
    pub dry_btn: GodotNodeHandle,
    #[node("Margin/VBox/Split/LogPanel/Log")]
    pub log: GodotNodeHandle,
}

/// 解析到的控件句柄（供 observer 读写节点）。
#[derive(Resource, Default)]
pub struct UiRefs {
    pub nodes: Option<UiNodes>,
}
