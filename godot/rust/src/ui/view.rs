//! 类型化节点视图与句柄资源。

use bevy::prelude::{Component, Resource};
use godot_bevy::interop::GodotNodeHandle;
use godot_bevy::prelude::NodeTreeView;

/// 运行台宿主实体标记（便于等待 GodotScene 生成完成）。
#[derive(Component)]
pub struct UiHost;

/// ui.tscn 的类型化视图。
#[derive(NodeTreeView)]
pub struct UiNodes {
    #[node("Margin/VBox/PathRow/SchemePath")]
    pub scheme_path: GodotNodeHandle,
    #[node("Margin/VBox/PathRow/MonthDir")]
    pub month_dir: GodotNodeHandle,
    #[node("Margin/VBox/ActionRow/ValidateBtn")]
    pub validate_btn: GodotNodeHandle,
    #[node("Margin/VBox/ActionRow/DryRunBtn")]
    pub dry_btn: GodotNodeHandle,
    #[node("Margin/VBox/Log")]
    pub log: GodotNodeHandle,
}

/// 解析到的控件句柄（供 observer 读写节点）。
#[derive(Resource, Default)]
pub struct UiRefs {
    pub nodes: Option<UiNodes>,
}
