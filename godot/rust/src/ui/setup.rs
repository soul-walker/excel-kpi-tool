//! 运行台页：等页面场景生成后，用 NodeTreeView 把子节点句柄解析到 `UiRefs`，
//! 并初始化方案下拉与月份目录默认值。

use bevy::ecs::query::With;
use bevy::prelude::{Query, ResMut};
use godot::classes::{Control, Label, OptionButton, ProjectSettings};
use godot::obj::Singleton;
use godot_bevy::prelude::{GodotAccess, GodotNodeHandle, NodeTreeView};
use tracing::info;

use super::schemes;
use super::view::{UiHost, UiNodes, UiRefs};

/// 每帧重试直到 NodeTreeView 成功解析当前页面节点，然后初始化控件。
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
                init_run_page(&mut godot, &nodes);
                refs.nodes = Some(nodes);
                return;
            }
            Err(_) => continue,
        }
    }
}

/// 填充方案下拉 + 月份目录默认值（开发默认指向仓库 testdata）。
fn init_run_page(godot: &mut GodotAccess, nodes: &UiNodes) {
    // 方案下拉
    let names = schemes::scheme_names();
    let mut select = godot.get::<OptionButton>(nodes.scheme_select);
    select.clear();
    for name in &names {
        select.add_item(name);
    }
    let preferred = names.iter().position(|n| n.contains("复刻")).unwrap_or(0);
    if !names.is_empty() {
        select.select(preferred as i32);
    }

    // 月份目录：默认指向仓库测试月目录（正式使用通过 FileDialog 选择）。
    let default_month = ProjectSettings::singleton()
        .globalize_path("res://../testdata/2023-09")
        .to_string();
    godot
        .get::<Label>(nodes.month_dir_label)
        .set_text(&default_month);
}
