//! 生成 ui.tscn（含按钮信号连接），并把子节点句柄解析到 `UiRefs`。

use bevy::ecs::query::With;
use bevy::prelude::{Commands, Name, Query, ResMut};
use godot::classes::Control;
use godot_bevy::prelude::{GodotAccess, GodotNodeHandle, GodotScene, NodeTreeView};
use tracing::{info, warn};

use super::events::{RunClicked, ValidateClicked};
use super::view::{UiHost, UiNodes, UiRefs};

/// 启动时把 ui.tscn 生成到主场景的 Main 宿主节点下，并连接按钮 pressed 信号。
pub fn spawn_ui(mut commands: Commands, hosts: Query<(&GodotNodeHandle, &Name)>) {
    let Some((host, _)) = hosts.iter().find(|(_, n)| n.as_str() == "Main") else {
        warn!("主场景未找到 Main 宿主节点");
        return;
    };
    info!("生成 ui.tscn → Main 宿主");

    let scene = GodotScene::from_path("res://ui.tscn")
        .with_parent(*host)
        .with_signal_connection(
            "Margin/VBox/ActionRow/ValidateBtn",
            "pressed",
            |_args, _node, _entity| Some(ValidateClicked),
        )
        .with_signal_connection(
            "Margin/VBox/ActionRow/DryRunBtn",
            "pressed",
            |_args, _node, _entity| Some(RunClicked { dry_run: true }),
        )
        .with_signal_connection(
            "Margin/VBox/ActionRow/RunBtn",
            "pressed",
            |_args, _node, _entity| Some(RunClicked { dry_run: false }),
        );

    commands.spawn((UiHost, scene));
}

/// 等 GodotScene 生成的场景可用后，用 NodeTreeView 取子节点句柄（每帧重试直到成功）。
pub fn resolve_ui_refs(
    roots: Query<&GodotNodeHandle, With<UiHost>>,
    mut refs: ResMut<UiRefs>,
    mut godot: GodotAccess,
) {
    if refs.nodes.is_some() {
        return;
    }
    for handle in &roots {
        let Some(control) = godot.try_get::<Control>(*handle) else {
            continue; // 场景尚未实例化完成
        };
        match UiNodes::from_node(control) {
            Ok(nodes) => {
                info!("NodeTreeView 解析成功（scheme/log/buttons 就绪）");
                refs.nodes = Some(nodes);
                return;
            }
            Err(_) => continue,
        }
    }
}
