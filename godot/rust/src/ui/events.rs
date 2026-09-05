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

/// 顶栏“方案编辑”导航。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct ShowEditorPage;

/// 编辑器：方案下拉切换（切换即载入当前项）。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct EditorSchemePicked;

/// 编辑器：“载入”按钮。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct EditorLoadClicked;

/// 编辑器：“保存”按钮。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct EditorSaveClicked;

/// 编辑器：“校验”按钮。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct EditorValidateClicked;

/// 编辑器：“新建方案”按钮。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct EditorNewSchemeClicked;

/// 编辑器：“+ 新增参数”按钮。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct EditorAddParamClicked;

/// 编辑器：删除第 index 个参数（参数行的删除按钮）。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct ParamRemoveClicked {
    pub index: i32,
}
