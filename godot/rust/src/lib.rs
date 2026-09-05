//! kpi_tool：Godot 4 + godot-bevy 客户端（方案编辑器 + 运行台）。
//!
//! 本文件只做 crate 根与入口装配，不含业务/UI 逻辑：
//! - `bridge`：调用 kpi-engine（headless），纯函数可单测；
//! - `ui`：Godot UI 场景桥接（视图/信号/事件/开发自检）。
//!
//! 约定（docs/tech.md）：不使用 GDScript；场景只含纯控件。

mod bridge;
mod ui;

use bevy::prelude::*;
use godot::prelude::*;
use godot_bevy::prelude::*;
use ui::UiPlugin;

#[bevy_app]
fn build_app(app: &mut App) {
    app.add_plugins(UiPlugin);
}
