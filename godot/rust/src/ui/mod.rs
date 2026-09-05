//! UI 模块：主场景壳 + 功能页路由 + 运行台页桥接。
//!
//! - `events`：按钮/导航 typed 事件；
//! - `view`：类型化节点视图（NodeTreeView）与句柄资源；
//! - `nav`：主场景路由（顶栏导航 → 页面场景的生成/切换）；
//! - `setup`：运行台场景节点句柄解析；
//! - `handlers`：运行台事件处理（读输入 → 引擎 → 日志）；
//! - `devshot`：开发自检（KPI_SHOT=1 截图）。

pub mod devshot;
pub mod events;
pub mod handlers;
pub mod nav;
pub mod setup;
pub mod view;

use bevy::app::{App, Plugin};
use events::{RunClicked, ShowRunPage, ValidateClicked};
use godot_bevy::prelude::{GodotDefaultPlugins, GodotSignalsPlugin};
use view::UiRefs;

/// UI 总插件（lib.rs 只负责装配它）。
pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(GodotDefaultPlugins)
            .add_plugins(GodotSignalsPlugin::<ValidateClicked>::default())
            .add_plugins(GodotSignalsPlugin::<RunClicked>::default())
            .add_plugins(GodotSignalsPlugin::<ShowRunPage>::default());

        app.init_resource::<UiRefs>();
        app.init_resource::<nav::ContentHost>();
        app.init_resource::<nav::ActivePage>();

        app.add_systems(bevy::prelude::Startup, nav::setup_router);
        app.add_systems(
            bevy::prelude::Update,
            (setup::resolve_ui_refs, devshot::dev_shot_system),
        );

        app.add_observer(handlers::on_validate_clicked);
        app.add_observer(handlers::on_run_clicked);
        app.add_observer(nav::on_show_run_page);
    }
}
