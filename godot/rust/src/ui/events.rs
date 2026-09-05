//! 按钮 pressed → Bevy 事件（由 typed signal 桥接映射）。

use bevy::prelude::Event;

/// “校验方案”按钮。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct ValidateClicked;

/// “Dry-run / 实际运行”按钮。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct RunClicked {
    pub dry_run: bool,
}

/// 顶栏“运行台”导航。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct ShowRunPage;
