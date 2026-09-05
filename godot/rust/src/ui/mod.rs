//! UI 总插件：把 Godot 场景（ui.tscn）与 Bevy ECS 接起来。
//! 只做“装配”，不含逻辑。

pub mod devshot;
pub mod events;
pub mod handlers;
pub mod setup;
pub mod view;

use bevy::app::{App, Plugin};
use events::{RunClicked, ValidateClicked};
use godot_bevy::prelude::{GodotDefaultPlugins, GodotSignalsPlugin};
use view::UiRefs;

/// UI 总插件（lib.rs 只负责装配它）。
pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(GodotDefaultPlugins)
            .add_plugins(GodotSignalsPlugin::<ValidateClicked>::default())
            .add_plugins(GodotSignalsPlugin::<RunClicked>::default());

        app.init_resource::<UiRefs>();

        app.add_systems(bevy::prelude::Startup, setup::spawn_ui);
        app.add_systems(
            bevy::prelude::Update,
            (setup::resolve_ui_refs, devshot::dev_shot_system),
        );

        app.add_observer(handlers::on_validate_clicked);
        app.add_observer(handlers::on_run_clicked);
    }
}
