//! 按钮 pressed → Bevy 事件（由 `GodotScene::with_signal_connection` 映射）。

use bevy::prelude::Event;

/// “校验方案”按钮。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct ValidateClicked;

/// “Dry-run / 实际运行”按钮。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct RunClicked {
    pub dry_run: bool,
}
