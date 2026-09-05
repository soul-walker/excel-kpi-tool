//! 页面路由：主场景（main.tscn）顶栏按钮 → 显示/切换功能页场景。
//! 运行台 → scenes/run/run_console.tscn；方案编辑 → scenes/editor/scheme_editor.tscn。

use bevy::ecs::observer::On;
use bevy::prelude::{Commands, Component, Entity, Name, Query, Res, ResMut, Resource};
use godot::builtin::GString;
use godot_bevy::prelude::{GodotNodeHandle, GodotScene, GodotSignals};
use tracing::{info, warn};

use super::derived::{DerivedDetail as DvDetail, DerivedListRows as DvList};
use super::editor::{EditorDoc, EditorRefs, EditorRoot, ParamRows};
use super::events::{
    AddColClicked, AddDerivedClicked, AddDerivedFieldClicked, AddDerivedMapClicked,
    AddSourceClicked, BrowseMonthClicked, EditorAddParamClicked, EditorLoadClicked,
    EditorNewSchemeClicked, EditorSaveClicked, EditorSchemePicked, EditorValidateClicked,
    MonthDirPicked, RunClicked, ShowEditorPage, ShowRunPage, ValidateClicked,
};
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
    mut dlist: bevy::prelude::NonSendMut<DvList>,
    mut ddetail: bevy::prelude::NonSendMut<DvDetail>,
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
    dlist.clear();
    ddetail.clear();
    spawn_run_page(&mut commands, content.0, &mut active);
}

/// 顶栏“运行台”被点击 → 显示运行台页。
pub fn on_show_run_page(
    _trigger: On<ShowRunPage>,
    mut commands: Commands,
    content: Res<ContentHost>,
    mut active: ResMut<ActivePage>,
    mut refs: ResMut<UiRefs>,
) {
    info!("导航：运行台");
    refs.nodes = None; // 让 resolve 重新绑定新页面节点
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
    mut dlist: bevy::prelude::NonSendMut<DvList>,
    mut ddetail: bevy::prelude::NonSendMut<DvDetail>,
) {
    info!("导航：方案编辑");
    editor_refs.nodes = None;
    editor_doc.path = None;
    editor_doc.base = None;
    editor_doc.raw = None;
    param_rows.clear();
    src_rows.clear();
    col_rows.clear();
    dlist.clear();
    ddetail.clear();
    spawn_editor_page(&mut commands, content.0, &mut active);
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
            "Margin/VBox/Split/LeftPanel/LeftVBox/Tabs/派生/DetailScroll/DetailVBox/MapHead/AddMapBtn",
            "pressed",
            |_args, _node, _entity| Some(AddDerivedMapClicked),
        )
        .with_signal_connection(
            "Margin/VBox/Split/LeftPanel/LeftVBox/Tabs/派生/DetailScroll/DetailVBox/FieldHead/AddFieldBtn",
            "pressed",
            |_args, _node, _entity| Some(AddDerivedFieldClicked),
        );
    let entity = commands.spawn((EditorRoot, PageRoot, scene)).id();
    active.0 = Some(entity);
}
