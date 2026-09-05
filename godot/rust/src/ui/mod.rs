//! UI 模块：主场景壳 + 功能页路由 + 运行台/方案编辑器桥接。
//!
//! - `events`：按钮/导航 typed 事件；
//! - `view`：类型化节点视图（NodeTreeView）与句柄资源；
//! - `schemes`：用户方案库（user://schemes）位置与播种；
//! - `editor`：方案编辑器（参数编辑/保存/校验）；
//! - `sources`：编辑器数据源/列映射页签；
//! - `nav`：主场景路由（顶栏导航 → 页面场景的生成/切换）；
//! - `setup`：运行台场景节点句柄解析与初始化；
//! - `handlers`：运行台事件处理（读输入 → 引擎 → 日志）；
//! - `devshot`：开发自检（KPI_SHOT=1 截图）。

pub mod derived;
pub mod devshot;
pub mod editor;
pub mod events;
pub mod handlers;
pub mod nav;
pub mod schemes;
pub mod setup;
pub mod sources;
pub mod view;

use bevy::app::{App, Plugin};
use events::{
    AddColClicked, AddDerivedClicked, AddDerivedFieldClicked, AddDerivedMapClicked,
    AddSourceClicked, BrowseMonthClicked, ColRemoveClicked, DerivedDetailClicked,
    DerivedFieldRemoveClicked, DerivedMapRemoveClicked, DerivedRemoveClicked,
    EditorAddParamClicked, EditorLoadClicked, EditorNewSchemeClicked, EditorSaveClicked,
    EditorSchemePicked, EditorValidateClicked, MonthDirPicked, ParamRemoveClicked, ParamTypeChosen,
    ParamValueEdited, RunClicked, ShowEditorPage, ShowRunPage, SourceColsClicked,
    SourceRemoveClicked, ValidateClicked,
};
use godot_bevy::prelude::{GodotDefaultPlugins, GodotSignalsPlugin};
use view::UiRefs;

/// UI 总插件（lib.rs 只负责装配它）。
pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(GodotDefaultPlugins)
            .add_plugins(GodotSignalsPlugin::<ValidateClicked>::default())
            .add_plugins(GodotSignalsPlugin::<RunClicked>::default())
            .add_plugins(GodotSignalsPlugin::<ShowRunPage>::default())
            .add_plugins(GodotSignalsPlugin::<BrowseMonthClicked>::default())
            .add_plugins(GodotSignalsPlugin::<MonthDirPicked>::default())
            .add_plugins(GodotSignalsPlugin::<ShowEditorPage>::default())
            .add_plugins(GodotSignalsPlugin::<EditorSchemePicked>::default())
            .add_plugins(GodotSignalsPlugin::<EditorLoadClicked>::default())
            .add_plugins(GodotSignalsPlugin::<EditorSaveClicked>::default())
            .add_plugins(GodotSignalsPlugin::<EditorValidateClicked>::default())
            .add_plugins(GodotSignalsPlugin::<EditorNewSchemeClicked>::default())
            .add_plugins(GodotSignalsPlugin::<EditorAddParamClicked>::default())
            .add_plugins(GodotSignalsPlugin::<ParamRemoveClicked>::default())
            .add_plugins(GodotSignalsPlugin::<ParamValueEdited>::default())
            .add_plugins(GodotSignalsPlugin::<ParamTypeChosen>::default())
            .add_plugins(GodotSignalsPlugin::<AddSourceClicked>::default())
            .add_plugins(GodotSignalsPlugin::<SourceRemoveClicked>::default())
            .add_plugins(GodotSignalsPlugin::<SourceColsClicked>::default())
            .add_plugins(GodotSignalsPlugin::<AddColClicked>::default())
            .add_plugins(GodotSignalsPlugin::<ColRemoveClicked>::default())
            .add_plugins(GodotSignalsPlugin::<AddDerivedClicked>::default())
            .add_plugins(GodotSignalsPlugin::<DerivedRemoveClicked>::default())
            .add_plugins(GodotSignalsPlugin::<DerivedDetailClicked>::default())
            .add_plugins(GodotSignalsPlugin::<AddDerivedMapClicked>::default())
            .add_plugins(GodotSignalsPlugin::<DerivedMapRemoveClicked>::default())
            .add_plugins(GodotSignalsPlugin::<AddDerivedFieldClicked>::default())
            .add_plugins(GodotSignalsPlugin::<DerivedFieldRemoveClicked>::default());

        app.init_resource::<UiRefs>();
        app.init_resource::<editor::EditorRefs>();
        app.init_resource::<editor::EditorDoc>();
        app.init_resource::<nav::ContentHost>();
        app.init_resource::<nav::ActivePage>();
        app.insert_non_send_resource(editor::ParamRows::default());
        app.insert_non_send_resource(sources::SourceRows::default());
        app.insert_non_send_resource(sources::ColRows::default());
        app.insert_non_send_resource(derived::DerivedListRows::default());
        app.insert_non_send_resource(derived::DerivedDetail::default());

        app.add_systems(bevy::prelude::Startup, nav::setup_router);
        app.add_systems(
            bevy::prelude::Update,
            (
                setup::resolve_ui_refs,
                editor::resolve_editor,
                editor::connect_param_signals,
                sources::connect_sources_signals,
                derived::connect_derived_signals,
                devshot::dev_shot_system,
            ),
        );

        app.add_observer(handlers::on_validate_clicked);
        app.add_observer(handlers::on_run_clicked);
        app.add_observer(handlers::on_browse_month);
        app.add_observer(handlers::on_month_dir_picked);
        app.add_observer(editor::on_load_clicked);
        app.add_observer(editor::on_scheme_picked);
        app.add_observer(editor::on_save_clicked);
        app.add_observer(editor::on_validate_clicked);
        app.add_observer(editor::on_new_scheme_clicked);
        app.add_observer(editor::on_add_param_clicked);
        app.add_observer(editor::on_remove_param);
        app.add_observer(editor::on_value_edited);
        app.add_observer(editor::on_type_chosen);
        app.add_observer(sources::on_add_source);
        app.add_observer(sources::on_remove_source);
        app.add_observer(sources::on_source_cols);
        app.add_observer(sources::on_add_col);
        app.add_observer(sources::on_remove_col);
        app.add_observer(derived::on_add_derived);
        app.add_observer(derived::on_remove_derived);
        app.add_observer(derived::on_derived_detail);
        app.add_observer(derived::on_add_map);
        app.add_observer(derived::on_remove_map);
        app.add_observer(derived::on_add_field);
        app.add_observer(derived::on_remove_field);
        app.add_observer(nav::on_show_run_page);
        app.add_observer(nav::on_show_editor_page);
    }
}
