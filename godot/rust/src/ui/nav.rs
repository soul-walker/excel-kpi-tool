//! 页面路由：主场景（main.tscn）顶栏按钮 → 显示/切换功能页场景。
//! 当前只有“运行台”页；新增页时在 events 加导航事件、在顶栏加按钮、在 spawn_<page> 加场景即可。

use bevy::ecs::observer::On;
use bevy::prelude::{Commands, Component, Entity, Name, Query, Res, ResMut, Resource};
use godot_bevy::prelude::{GodotNodeHandle, GodotScene, GodotSignals};
use tracing::{info, warn};

use super::events::ShowRunPage;
use super::view::{UiHost, UiRefs};

/// 内容宿主（main.tscn 的 Content 节点），页面场景都挂到这里。
#[derive(Resource, Default)]
pub struct ContentHost(pub Option<GodotNodeHandle>);

/// 当前活动页面实体（用于切页时先卸载旧的）。
#[derive(Resource, Default)]
pub struct ActivePage(pub Option<Entity>);

/// 页面实体标记。
#[derive(Component)]
pub struct PageRoot;

/// 启动：找到 Content 宿主、连好顶栏导航按钮，并进入默认页（运行台）。
pub fn setup_router(
    mut commands: Commands,
    nodes: Query<(&GodotNodeHandle, &Name)>,
    nav_signals: GodotSignals<ShowRunPage>,
    mut content: ResMut<ContentHost>,
    mut active: ResMut<ActivePage>,
    mut refs: ResMut<UiRefs>,
) {
    for (handle, name) in &nodes {
        match name.as_str() {
            "Content" => content.0 = Some(*handle),
            "RunNavBtn" => {
                nav_signals.connect(*handle, "pressed", None, |_a, _n, _e| Some(ShowRunPage));
            }
            _ => {}
        }
    }
    info!("路由初始化完成，进入默认页：运行台");
    refs.nodes = None;
    spawn_run_page(&mut commands, content.0, &mut active);
}

/// 顶栏“运行台”被点击 → 显示运行台页。
pub fn on_show_run_page(
    _trigger: On<ShowRunPage>,
    mut commands: Commands,
    content: Res<ContentHost>,
    mut active: ResMut<ActivePage>,
    mut refs: ResMut<UiRefs>,
) {
    info!("导航：运行台");
    refs.nodes = None; // 让 resolve 重新绑定新页面节点
    spawn_run_page(&mut commands, content.0, &mut active);
}

fn spawn_run_page(commands: &mut Commands, host: Option<GodotNodeHandle>, active: &mut ActivePage) {
    let Some(host) = host else {
        warn!("找不到 Content 宿主，无法挂载运行台页");
        return;
    };
    if let Some(entity) = active.0.take() {
        commands.entity(entity).despawn();
    }
    let entity = commands
        .spawn((
            UiHost,
            PageRoot,
            GodotScene::from_path("res://scenes/run/run_console.tscn").with_parent(host),
        ))
        .id();
    active.0 = Some(entity);
}
