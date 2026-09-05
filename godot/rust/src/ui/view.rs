//! 类型化节点视图与句柄资源。

use bevy::prelude::{Component, Resource};
use godot_bevy::interop::GodotNodeHandle;
use godot_bevy::prelude::NodeTreeView;

/// 页面实体标记（供路由切页）。
#[derive(Component)]
pub struct UiHost;

/// run_console.tscn 的类型化视图。
#[derive(NodeTreeView)]
pub struct UiNodes {
    #[node("Margin/VBox/SchemeRow/SchemeSelect")]
    pub scheme_select: GodotNodeHandle,
    #[node("Margin/VBox/MonthRow/MonthDirLabel")]
    pub month_dir_label: GodotNodeHandle,
    #[node("MonthDirDialog")]
    pub month_dialog: GodotNodeHandle,
    #[node("Margin/VBox/ActionRow/ValidateBtn")]
    pub validate_btn: GodotNodeHandle,
    #[node("Margin/VBox/ActionRow/DryRunBtn")]
    pub dry_btn: GodotNodeHandle,
    #[node("Margin/VBox/Split/LogPanel/Log")]
    pub log: GodotNodeHandle,
}

/// 解析到的控件句柄（供 observer 读写节点）。
#[derive(Resource, Default)]
pub struct UiRefs {
    pub nodes: Option<UiNodes>,
}
