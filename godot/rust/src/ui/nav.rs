//! 页面路由：主场景（main.tscn）顶栏按钮 → 显示/切换功能页场景。
//! 运行台 → scenes/run/run_console.tscn；方案编辑 → scenes/editor/scheme_editor.tscn。

use bevy::ecs::observer::On;
use bevy::prelude::{Commands, Component, Entity, Name, Query, Res, ResMut, Resource};
use godot::builtin::{GString, Vector2, Vector2i};
use godot::classes::{Control, RichTextLabel, Window};
use godot_bevy::prelude::{GodotAccess, GodotNodeHandle, GodotScene, GodotSignals};
use tracing::{info, warn};

use super::aggregates::AggregateDetail as AggDetail;
use super::derived::DerivedDetail as DvDetail;
use super::editor::{EditorDoc, EditorRefs, EditorRoot, ParamRows};
use super::events::{
    AddAggregateClicked, AddColClicked, AddDerivedClicked, AddDerivedFieldClicked, AddGroupClicked,
    AddGroupItemClicked, AddOutputClicked, AddOutputColClicked, AddOutputNfColClicked,
    AddOutputSheetClicked, AddSourceClicked, AggFromChanged, AggregatePicked, BrowseMonthClicked,
    DerivedPicked, EditorAddParamClicked, EditorCloneClicked, EditorLoadClicked,
    EditorNewSchemeClicked, EditorSaveClicked, EditorSchemePicked, EditorValidateClicked,
    GroupPicked, MonthDirPicked, OutputFromChanged, OutputKindChanged, OutputPicked,
    RemoveCurrentAggregate, RemoveCurrentDerived, RemoveCurrentGroup, RemoveCurrentOutput,
    RunClicked, ShowEditorPage, ShowHelpClicked, ShowRunPage, ValidateClicked,
};
use super::groups::GroupDetail as GDetail;
use super::outputs::OutputDetail as OutDetail;
use super::sources::{ColRows as SrcCols, SourceRows as SrcRows};
use super::view::{UiHost, UiRefs};

/// 内容宿主（main.tscn 的 Content 节点），页面场景都挂到这里。
#[derive(Resource, Default)]
pub struct ContentHost(pub Option<GodotNodeHandle>);

/// 当前活动页面实体（用于切页时先卸载旧的）。
#[derive(Resource, Default)]
pub struct ActivePage(pub Option<Entity>);

/// 页面实体标记（路由用）。
#[derive(Component)]
pub struct PageRoot;

/// 主场景里「说明」窗口与正文节点句柄（main.tscn 常驻，随场景初始化）。
#[derive(Resource, Default)]
pub struct HelpRefs {
    pub window: Option<GodotNodeHandle>,
    pub text: Option<GodotNodeHandle>,
}

/// 顶栏「说明」弹窗内容（RichTextLabel，bbcode）。
const EXPR_HELP_TEXT: &str = "\
[b]表达式怎么写？——块语法速查[/b]\n\n\
表达式用在三个地方：派生视图的列表达式、聚合指标的 where / expr、计算分组的字段与覆盖。\n\n\
[b]1. 引用名字[/b]\n\
• 刚算好的字段：直接写名字，如 [code]课时费S + 课时费X[/code]\n\
• 方案参数：加 [code]参数.[/code] 前缀，如 [code]课时S * 参数.课时费[/code]\n\
• 聚合指标：加 [code]聚合.[/code] 前缀，如 [code]聚合.请假超额[/code]\n\
• 表里的列：写 [code]表id.列名[/code]，如 [code]keshi.课时[/code]、[code]qingjia.天数[/code]\n\n\
[b]2. 运算符[/b]\n\
算术 [code]+ - * /[/code]　比较 [code]= != > >= < <=[/code]　逻辑 [code]AND OR NOT[/code]\n\
例：[code](课时S - 参数.基础课时) > 0 AND NOT 聚合.请假超额[/code]\n\
（注意：不要写 [code]![/code] 取反，请用 [code]NOT[/code]）\n\n\
[b]3. 空值[/b]\n\
[code]a ?? b[/code]　a 为空/没取到时改用 b，如 [code]keshi.课时 ?? 0[/code]\n\
单问号 [code]a ? b[/code] 效果相同。\n\n\
[b]4. if 判断[/b]\n\
[code]if 条件 { 成立的结果 } else { 不成立的结果 }[/code]\n\
例：[code]if 聚合.请假超额 { 0 } else { 课时S * 参数.课时费 }[/code]\n\
• 多个分支用 [code]else if[/code] 继续接；省略 else 时“不成立”返回 空。\n\
• 旧 Excel 的 [code]IF(条件, 真, 假)[/code] 已移除，请勿再用。\n\n\
[b]5. 函数[/b]\n\
[code]MIN(a,b)[/code] 取较小　[code]MAX(a,b)[/code] 取较大　[code]ABS(x)[/code] 绝对值　[code]ROUND(x,n)[/code] 保留 n 位小数\n\
例：[code]ROUND(上浮30%, 0)[/code]\n\n\
[b]6. 对一组多行求汇总（派生 / 聚合内）[/b]\n\
[code]SUM(列) COUNT(列) AVG(列)[/code] … 如 [code]SUM(qingjia.天数)[/code]\n\
[code]GROUP_BY(列)[/code] 表示按该列把多行分到“一人一组”。\n\n\
[b]小提示[/b]\n\
• 表达式输入框里按 Ctrl+Space 可弹出补全。\n\
• 每页顶部带「？」的按钮，悬停可看该页写法的完整说明。";

/// 启动：找到 Content 宿主、连好顶栏导航按钮，并进入默认页（运行台）。
#[allow(clippy::too_many_arguments)]
pub fn setup_router(
    mut commands: Commands,
    nodes: Query<(&GodotNodeHandle, &Name)>,
    run_signals: GodotSignals<ShowRunPage>,
    editor_signals: GodotSignals<ShowEditorPage>,
    mut content: ResMut<ContentHost>,
    mut active: ResMut<ActivePage>,
    mut refs: ResMut<UiRefs>,
    mut editor_refs: ResMut<EditorRefs>,
    mut editor_doc: ResMut<EditorDoc>,
    mut param_rows: bevy::prelude::NonSendMut<ParamRows>,
    mut src_rows: bevy::prelude::NonSendMut<SrcRows>,
    mut col_rows: bevy::prelude::NonSendMut<SrcCols>,
    mut ddetail: bevy::prelude::NonSendMut<DvDetail>,
    mut adetail: bevy::prelude::NonSendMut<AggDetail>,
    mut gdetail: bevy::prelude::NonSendMut<GDetail>,
    mut odetail: bevy::prelude::NonSendMut<OutDetail>,
) {
    for (handle, name) in &nodes {
        match name.as_str() {
            "Content" => content.0 = Some(*handle),
            "RunNavBtn" => {
                run_signals.connect(*handle, "pressed", None, |_a, _n, _e| Some(ShowRunPage));
            }
            "EditorNavBtn" => {
                editor_signals.connect(*handle, "pressed", None, |_a, _n, _e| Some(ShowEditorPage));
            }
            _ => {}
        }
    }
    info!("路由初始化完成，进入默认页：运行台");
    refs.nodes = None;
    editor_refs.nodes = None;
    editor_doc.path = None;
    editor_doc.base = None;
    editor_doc.raw = None;
    param_rows.clear();
    src_rows.clear();
    col_rows.clear();
    ddetail.clear();
    adetail.clear();
    gdetail.clear();
    odetail.clear();
    spawn_run_page(&mut commands, content.0, &mut active);
}

/// 顶栏「说明」窗口的句柄捕获与按钮连接（独立 Startup 系统，避免 setup_router 参数超限）。
pub fn setup_help(
    nodes: Query<(&GodotNodeHandle, &Name)>,
    help_signals: GodotSignals<ShowHelpClicked>,
    mut help: ResMut<HelpRefs>,
) {
    for (handle, name) in &nodes {
        match name.as_str() {
            "HelpNavBtn" | "HelpCloseBtn" => {
                help_signals.connect(*handle, "pressed", None, |_a, _n, _e| Some(ShowHelpClicked));
            }
            "HelpWindow" => {
                help.window = Some(*handle);
                help_signals.connect(*handle, "close_requested", None, |_a, _n, _e| {
                    Some(ShowHelpClicked)
                });
            }
            "HelpText" => help.text = Some(*handle),
            _ => {}
        }
    }
}

/// 顶栏“运行台”被点击 → 显示运行台页。
/// 离开方案编辑器前清空编辑器各页的控件句柄资源，避免每帧补连/克隆已释放节点的 Gd。
#[allow(clippy::too_many_arguments)]
pub fn on_show_run_page(
    _trigger: On<ShowRunPage>,
    mut commands: Commands,
    content: Res<ContentHost>,
    mut active: ResMut<ActivePage>,
    mut refs: ResMut<UiRefs>,
    mut editor_refs: ResMut<EditorRefs>,
    mut param_rows: bevy::prelude::NonSendMut<ParamRows>,
    mut src_rows: bevy::prelude::NonSendMut<SrcRows>,
    mut col_rows: bevy::prelude::NonSendMut<SrcCols>,
    mut ddetail: bevy::prelude::NonSendMut<DvDetail>,
    mut adetail: bevy::prelude::NonSendMut<AggDetail>,
    mut gdetail: bevy::prelude::NonSendMut<GDetail>,
    mut odetail: bevy::prelude::NonSendMut<OutDetail>,
) {
    info!("导航：运行台");
    refs.nodes = None; // 让 resolve 重新绑定新页面节点
    editor_refs.nodes = None;
    param_rows.clear();
    src_rows.clear();
    col_rows.clear();
    ddetail.clear();
    adetail.clear();
    gdetail.clear();
    odetail.clear();
    spawn_run_page(&mut commands, content.0, &mut active);
}

/// 顶栏“方案编辑”被点击 → 显示方案编辑器页。
#[allow(clippy::too_many_arguments)]
pub fn on_show_editor_page(
    _trigger: On<ShowEditorPage>,
    mut commands: Commands,
    content: Res<ContentHost>,
    mut active: ResMut<ActivePage>,
    mut editor_refs: ResMut<EditorRefs>,
    mut editor_doc: ResMut<EditorDoc>,
    mut param_rows: bevy::prelude::NonSendMut<ParamRows>,
    mut src_rows: bevy::prelude::NonSendMut<SrcRows>,
    mut col_rows: bevy::prelude::NonSendMut<SrcCols>,
    mut ddetail: bevy::prelude::NonSendMut<DvDetail>,
    mut adetail: bevy::prelude::NonSendMut<AggDetail>,
    mut gdetail: bevy::prelude::NonSendMut<GDetail>,
    mut odetail: bevy::prelude::NonSendMut<OutDetail>,
) {
    info!("导航：方案编辑");
    editor_refs.nodes = None;
    editor_doc.path = None;
    editor_doc.base = None;
    editor_doc.raw = None;
    param_rows.clear();
    src_rows.clear();
    col_rows.clear();
    ddetail.clear();
    adetail.clear();
    gdetail.clear();
    odetail.clear();
    spawn_editor_page(&mut commands, content.0, &mut active);
}

/// 顶栏「说明」/弹窗「关闭 / X」：切换表达式说明窗口的显隐。
pub fn on_show_help(
    _trigger: On<ShowHelpClicked>,
    help: Res<HelpRefs>,
    content: Res<ContentHost>,
    mut godot: GodotAccess,
) {
    let Some(window) = help.window else {
        warn!("找不到说明窗口节点");
        return;
    };
    let mut w = godot.get::<Window>(window);
    if w.is_visible() {
        w.hide();
        return;
    }
    if let Some(text) = help.text {
        godot.get::<RichTextLabel>(text).set_text(EXPR_HELP_TEXT);
    }
    // 以主界面内容控件（Content，铺满根窗口）的逻辑尺寸为基准，取 80% 宽高，
    // 避免用根窗口 get_size() 在高 DPI/编辑器内嵌时拿到放大的物理像素。
    let base: Vector2 = content
        .0
        .map(|h| godot.get::<Control>(h).get_size())
        .unwrap_or(Vector2::new(1280.0, 720.0));
    let size_x = ((base.x * 0.8).round() as i32)
        .max(700)
        .min(base.x as i32)
        .max(1);
    let size_y = ((base.y * 0.8).round() as i32)
        .max(480)
        .min(base.y as i32)
        .max(1);
    w.set_size(Vector2i::new(size_x, size_y));
    w.popup_centered();
}

fn spawn_run_page(commands: &mut Commands, host: Option<GodotNodeHandle>, active: &mut ActivePage) {
    let Some(host) = host else {
        warn!("找不到 Content 宿主，无法挂载运行台页");
        return;
    };
    if let Some(entity) = active.0.take() {
        commands.entity(entity).despawn();
    }
    let scene = GodotScene::from_path("res://scenes/run/run_console.tscn")
        .with_parent(host)
        .with_signal_connection(
            "Margin/VBox/Split/ControlsBox/ActionRow/ValidateBtn",
            "pressed",
            |_args, _node, _entity| Some(ValidateClicked),
        )
        .with_signal_connection(
            "Margin/VBox/Split/ControlsBox/ActionRow/DryRunBtn",
            "pressed",
            |_args, _node, _entity| Some(RunClicked { dry_run: true }),
        )
        .with_signal_connection(
            "Margin/VBox/Split/ControlsBox/ActionRow/RunBtn",
            "pressed",
            |_args, _node, _entity| Some(RunClicked { dry_run: false }),
        )
        .with_signal_connection(
            "Margin/VBox/Split/ControlsBox/MonthRow/MonthPickBtn",
            "pressed",
            |_args, _node, _entity| Some(BrowseMonthClicked),
        )
        .with_signal_connection("MonthDirDialog", "dir_selected", |args, _node, _entity| {
            let dir = args
                .first()
                .and_then(|v| v.try_to::<GString>().ok())
                .map(|s| s.to_string())
                .unwrap_or_default();
            Some(MonthDirPicked { dir })
        });
    let entity = commands.spawn((UiHost, PageRoot, scene)).id();
    active.0 = Some(entity);
}

fn spawn_editor_page(
    commands: &mut Commands,
    host: Option<GodotNodeHandle>,
    active: &mut ActivePage,
) {
    let Some(host) = host else {
        warn!("找不到 Content 宿主，无法挂载方案编辑页");
        return;
    };
    if let Some(entity) = active.0.take() {
        commands.entity(entity).despawn();
    }
    let scene = GodotScene::from_path("res://scenes/editor/scheme_editor.tscn")
        .with_parent(host)
        .with_signal_connection(
            "Margin/VBox/Toolbar/SchemeSelect",
            "item_selected",
            |args, _node, _entity| {
                let _ = args.first();
                Some(EditorSchemePicked)
            },
        )
        .with_signal_connection(
            "Margin/VBox/Toolbar/NewBtn",
            "pressed",
            |_args, _node, _entity| Some(EditorNewSchemeClicked),
        )
        .with_signal_connection(
            "Margin/VBox/Toolbar/CloneBtn",
            "pressed",
            |_args, _node, _entity| Some(EditorCloneClicked),
        )
        .with_signal_connection(
            "Margin/VBox/Toolbar/LoadBtn",
            "pressed",
            |_args, _node, _entity| Some(EditorLoadClicked),
        )
        .with_signal_connection(
            "Margin/VBox/Toolbar/SaveBtn",
            "pressed",
            |_args, _node, _entity| Some(EditorSaveClicked),
        )
        .with_signal_connection(
            "Margin/VBox/Toolbar/ValidateBtn",
            "pressed",
            |_args, _node, _entity| Some(EditorValidateClicked),
        )
        .with_signal_connection(
            "Margin/VBox/Split/LeftPanel/LeftVBox/Tabs/参数/ParamsTitleRow/AddParamBtn",
            "pressed",
            |_args, _node, _entity| Some(EditorAddParamClicked),
        )
        .with_signal_connection(
            "Margin/VBox/Split/LeftPanel/LeftVBox/Tabs/数据源/SourcesTitleRow/AddSourceBtn",
            "pressed",
            |_args, _node, _entity| Some(AddSourceClicked),
        )
        .with_signal_connection(
            "Margin/VBox/Split/LeftPanel/LeftVBox/Tabs/数据源/ColsTitleRow/AddColBtn",
            "pressed",
            |_args, _node, _entity| Some(AddColClicked),
        )
        .with_signal_connection(
            "Margin/VBox/Split/LeftPanel/LeftVBox/Tabs/派生/DerivedHintRow/AddDerivedBtn",
            "pressed",
            |_args, _node, _entity| Some(AddDerivedClicked),
        )
        .with_signal_connection(
            "Margin/VBox/Split/LeftPanel/LeftVBox/Tabs/派生/PickRow/DerivedPick",
            "item_selected",
            |_args, _node, _entity| Some(DerivedPicked),
        )
        .with_signal_connection(
            "Margin/VBox/Split/LeftPanel/LeftVBox/Tabs/派生/PickRow/DelDerivedBtn",
            "pressed",
            |_args, _node, _entity| Some(RemoveCurrentDerived),
        )
        .with_signal_connection(
            "Margin/VBox/Split/LeftPanel/LeftVBox/Tabs/派生/DetailScroll/DetailVBox/FieldHead/AddFieldBtn",
            "pressed",
            |_args, _node, _entity| Some(AddDerivedFieldClicked),
        )
        .with_signal_connection(
            "Margin/VBox/Split/LeftPanel/LeftVBox/Tabs/聚合指标/AggHintRow/AddAggBtn",
            "pressed",
            |_args, _node, _entity| Some(AddAggregateClicked),
        )
        .with_signal_connection(
            "Margin/VBox/Split/LeftPanel/LeftVBox/Tabs/聚合指标/PickRow/AggPick",
            "item_selected",
            |_args, _node, _entity| Some(AggregatePicked),
        )
        .with_signal_connection(
            "Margin/VBox/Split/LeftPanel/LeftVBox/Tabs/聚合指标/PickRow/DelAggBtn",
            "pressed",
            |_args, _node, _entity| Some(RemoveCurrentAggregate),
        )
        .with_signal_connection(
            "Margin/VBox/Split/LeftPanel/LeftVBox/Tabs/聚合指标/DetailScroll/DetailVBox/FromRow/AggFromSelect",
            "item_selected",
            |_args, _node, _entity| Some(AggFromChanged),
        )
        .with_signal_connection(
            "Margin/VBox/Split/LeftPanel/LeftVBox/Tabs/计算分组/GroupHintRow/AddGroupBtn",
            "pressed",
            |_args, _node, _entity| Some(AddGroupClicked),
        )
        .with_signal_connection(
            "Margin/VBox/Split/LeftPanel/LeftVBox/Tabs/计算分组/PickRow/GroupPick",
            "item_selected",
            |_args, _node, _entity| Some(GroupPicked),
        )
        .with_signal_connection(
            "Margin/VBox/Split/LeftPanel/LeftVBox/Tabs/计算分组/PickRow/DelGroupBtn",
            "pressed",
            |_args, _node, _entity| Some(RemoveCurrentGroup),
        )
        .with_signal_connection(
            "Margin/VBox/Split/LeftPanel/LeftVBox/Tabs/计算分组/DetailScroll/DetailVBox/ItemHead/AddItemBtn",
            "pressed",
            |_args, _node, _entity| Some(AddGroupItemClicked),
        )
        .with_signal_connection(
            "Margin/VBox/Split/LeftPanel/LeftVBox/Tabs/输出/OutHintRow/AddOutputBtn",
            "pressed",
            |_args, _node, _entity| Some(AddOutputClicked),
        )
        .with_signal_connection(
            "Margin/VBox/Split/LeftPanel/LeftVBox/Tabs/输出/PickRow/OutputPick",
            "item_selected",
            |_args, _node, _entity| Some(OutputPicked),
        )
        .with_signal_connection(
            "Margin/VBox/Split/LeftPanel/LeftVBox/Tabs/输出/PickRow/DelOutputBtn",
            "pressed",
            |_args, _node, _entity| Some(RemoveCurrentOutput),
        )
        .with_signal_connection(
            "Margin/VBox/Split/LeftPanel/LeftVBox/Tabs/输出/DetailScroll/DetailVBox/KindRow/OutputKindSelect",
            "item_selected",
            |_args, _node, _entity| Some(OutputKindChanged),
        )
        .with_signal_connection(
            "Margin/VBox/Split/LeftPanel/LeftVBox/Tabs/输出/DetailScroll/DetailVBox/NfBox/NfFromRow/NfFromSelect",
            "item_selected",
            |_args, _node, _entity| Some(OutputFromChanged),
        )
        .with_signal_connection(
            "Margin/VBox/Split/LeftPanel/LeftVBox/Tabs/输出/DetailScroll/DetailVBox/PatchBox/SheetHead/AddSheetBtn",
            "pressed",
            |_args, _node, _entity| Some(AddOutputSheetClicked),
        )
        .with_signal_connection(
            "Margin/VBox/Split/LeftPanel/LeftVBox/Tabs/输出/DetailScroll/DetailVBox/PatchBox/ColHead/AddColBtn",
            "pressed",
            |_args, _node, _entity| Some(AddOutputColClicked),
        )
        .with_signal_connection(
            "Margin/VBox/Split/LeftPanel/LeftVBox/Tabs/输出/DetailScroll/DetailVBox/NfBox/NfColHead/AddNfColBtn",
            "pressed",
            |_args, _node, _entity| Some(AddOutputNfColClicked),
        );
    let entity = commands.spawn((EditorRoot, PageRoot, scene)).id();
    active.0 = Some(entity);
}
