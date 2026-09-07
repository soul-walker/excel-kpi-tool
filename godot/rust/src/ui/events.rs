//! 按钮/控件事件（由 typed signal 桥接映射到 Bevy）。

use bevy::prelude::Event;
use godot_bevy::interop::GodotNodeHandle;

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

/// 顶栏“说明”按钮（弹出表达式关键字说明对话框）。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct ShowHelpClicked;

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

/// 编辑器：“克隆当前方案”按钮（以当前方案为蓝本建一份未保存草稿，供多版本微调）。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct EditorCloneClicked;

/// 编辑器：“+ 新增参数”按钮。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct EditorAddParamClicked;

/// 编辑器：删除第 index 个参数（参数行的删除按钮）。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct ParamRemoveClicked {
    pub index: i32,
}

/// 编辑器：参数值输入变化（用于自动推断类型）。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct ParamValueEdited {
    pub index: i32,
}

/// 编辑器：参数类型被手动选择（锁定为该行类型，不再自动推断）。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct ParamTypeChosen {
    pub index: i32,
}

/// 编辑器：“+ 新增数据源”。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct AddSourceClicked;

/// 编辑器：删除第 index 个数据源。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct SourceRemoveClicked {
    pub index: i32,
}

/// 编辑器：选中第 index 个数据源（“列映射”按钮）编辑其列。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct SourceColsClicked {
    pub index: i32,
}

/// 编辑器：给当前选中的数据源“+ 新增列”。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct AddColClicked;

/// 编辑器：删除当前数据源的第 index 列。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct ColRemoveClicked {
    pub index: i32,
}

/// 编辑器：“+ 新增派生视图”。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct AddDerivedClicked;

/// 编辑器：派生下拉框切换了要编辑的派生（目标序号 = 控件当前选中项）。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct DerivedPicked;

/// 编辑器：“删除当前派生”（删除下拉当前选中的派生）。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct RemoveCurrentDerived;

/// 编辑器：“+ 视图列”到当前派生。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct AddDerivedFieldClicked;

/// 编辑器：删除当前派生的第 index 个视图列。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct DerivedFieldRemoveClicked {
    pub index: i32,
}

/// 编辑器：“+ 新增聚合指标”。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct AddAggregateClicked;

/// 编辑器：聚合下拉框切换了要编辑的指标（目标序号 = 控件当前选中项）。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct AggregatePicked;

/// 编辑器：“删除当前指标”（删除下拉当前选中的聚合指标）。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct RemoveCurrentAggregate;

/// 编辑器：聚合指标详情里的“来源表 from”下拉被改动（分组列候选随之刷新）。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct AggFromChanged;

/// 编辑器：“+ 新增计算分组”。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct AddGroupClicked;

/// 编辑器：分组下拉框切换了要编辑的分组（目标序号 = 控件当前选中项）。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct GroupPicked;

/// 编辑器：“删除当前分组”（删除下拉当前选中的分组）。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct RemoveCurrentGroup;

/// 编辑器：给当前分组“+ 新增一行”（第 1 组 = 基础字段，其它组 = 覆盖）。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct AddGroupItemClicked;

/// 编辑器：删除当前分组的第 index 行（基础字段/覆盖行）。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct GroupItemRemoveClicked {
    pub index: i32,
}

/// 编辑器：“+ 新增输出”。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct AddOutputClicked;

/// 编辑器：输出下拉框切换了要编辑的输出（目标序号 = 控件当前选中项）。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct OutputPicked;

/// 编辑器：“删除当前输出”（删除下拉当前选中的输出）。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct RemoveCurrentOutput;

/// 编辑器：输出方式下拉被改动（重建详情面板；切换会清空原方式下的详细配置）。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct OutputKindChanged;

/// 编辑器：新文件输出的“数据来源 from”下拉被改动（列候选随之刷新）。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct OutputFromChanged;

/// 编辑器：回写模板的“+ 新增 sheet”（分组→sheet 映射行）。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct AddOutputSheetClicked;

/// 编辑器：删除当前输出的第 index 个分组→sheet 映射行。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct OutputSheetRemoveClicked {
    pub index: i32,
}

/// 编辑器：回写模板的“+ 新增写入列”。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct AddOutputColClicked;

/// 编辑器：删除回写模板第 index 个写入列。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct OutputColRemoveClicked {
    pub index: i32,
}

/// 编辑器：新文件输出的“+ 新增列”。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct AddOutputNfColClicked;

/// 编辑器：删除新文件输出第 index 个列。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct OutputNfColRemoveClicked {
    pub index: i32,
}

/// 表达式 CodeEdit：文本变化（用于按输入节奏请求补全）。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct ExprTextChanged {
    pub node: GodotNodeHandle,
}

/// 表达式 CodeEdit：请求补全（Ctrl+Space / `.` 前缀 / 输入中都会触发）。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct ExprCompletionRequested {
    pub node: GodotNodeHandle,
}

/// 表达式 CodeEdit：获得焦点（用于展开高度给补全列表留空间）。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct ExprFocusEntered {
    pub node: GodotNodeHandle,
}

/// 表达式 CodeEdit：失去焦点（恢复紧凑高度）。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct ExprFocusExited {
    pub node: GodotNodeHandle,
}

/// 命名类输入框（数据源 id / 列 alias / 派生 id / 视图列名 / 参数名）失去焦点。
/// 触发“改名即同步”：先落库再引用传播，其它页控件立即刷新。
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct NameFieldFocusLost {
    pub node: GodotNodeHandle,
}
