//! Bevy ECS 承载层：把 engine 作为 Plugin 挂到 Godot(godot-bevy) 或 headless App 上。
//!
//! 定稿决策：核心逻辑全部用 ECS（组件/资源/系统）实现；GUI 事件经 godot-bevy 进入 ECS。
//! 本文件当前只是骨架：注册资源与空启动系统；M1 开始填充编译/执行的状态机与系统。

use bevy::app::{App, Plugin};
use bevy::prelude::Resource;

/// 引擎已就绪标记（示例资源，后续可扩展为方案库/运行状态等 Resource）。
#[derive(Resource, Default)]
pub struct EngineReady;

/// engine 的 Bevy Plugin。
pub struct EnginePlugin;

impl Plugin for EnginePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(EngineReady);
        // TODO(M1): 注册方案加载/校验、DAG 执行、运行状态机等 System/Event/Resource。
    }
}
