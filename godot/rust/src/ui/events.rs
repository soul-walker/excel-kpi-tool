//! 按钮/控件事件（由 typed signal 桥接映射到 Bevy）。

use bevy::prelude::Event;

/// “校验方案”按钮。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct ValidateClicked;

/// “Dry-run / 实际运行”按钮。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct RunClicked {
    pub dry_run: bool,
}

/// “选择…”按钮（打开月份目录 FileDialog）。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct BrowseMonthClicked;

/// FileDialog 选定目录后触发。
#[derive(Event, Debug, Clone)]
pub(crate) struct MonthDirPicked {
    pub dir: String,
}

/// 顶栏“运行台”导航。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct ShowRunPage;
