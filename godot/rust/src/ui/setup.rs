//! 运行台页：等页面场景生成后，用 NodeTreeView 把子节点句柄解析到 `UiRefs`。

use bevy::ecs::query::With;
use bevy::prelude::{Query, ResMut};
use godot::classes::Control;
use godot_bevy::prelude::{GodotAccess, GodotNodeHandle, NodeTreeView};
use tracing::info;

use super::view::{UiHost, UiNodes, UiRefs};

/// 每帧重试直到 NodeTreeView 成功解析当前页面节点。
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
