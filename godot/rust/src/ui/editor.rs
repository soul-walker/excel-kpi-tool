//! 方案编辑器（v1：新建方案 + 名称/描述编辑 + 参数名称/类型/值增删改 + 保存/校验）。
//! 数据源/派生/聚合/计算组/输出等区块后续在同一骨架加入。
//!
//! 文档模型：`EditorDoc` 持有当前方案的目标路径与内存 JSON。
//! - 「新建方案」只建内存草稿（不写盘），名称预填 `未命名方案-<时间戳>`；
//! - 页面顶部名称/描述可直接编辑；保存时若名称与当前文件基名不同则重命名文件；
//! - 参数行：名称(LineEdit) / 类型(OptionButton) / 值(LineEdit) / 删除(Button)。
#![allow(clippy::too_many_arguments)]

use bevy::ecs::observer::On;
use bevy::ecs::query::With;
use bevy::prelude::{Component, Query, Res, ResMut, Resource};
use godot::builtin::Vector2;
use godot::classes::control::SizeFlags;
use godot::classes::{
    Button, Control, HBoxContainer, Label, LineEdit, OptionButton, RichTextLabel, VBoxContainer,
};
use godot::obj::NewAlloc;
use godot::prelude::Gd;
use godot_bevy::prelude::{GodotAccess, GodotNodeHandle, GodotSignals, NodeTreeView};
use tracing::info;

use crate::bridge;

use super::derived::{self, DerivedDetail as DvDetail, DerivedListRows as DvList};
use super::events::{
    EditorAddParamClicked, EditorLoadClicked, EditorNewSchemeClicked, EditorSaveClicked,
    EditorSchemePicked, EditorValidateClicked, ParamRemoveClicked, ParamTypeChosen,
    ParamValueEdited,
};
use super::schemes;
use super::sources::{self, ColRows as SrcCols, SourceRows as SrcRows};
use super::view::{LogKind, log_line};

/// 编辑器页面实体标记。
#[derive(Component)]
pub struct EditorRoot;

/// scheme_editor.tscn 的类型化视图。
#[derive(NodeTreeView)]
pub struct EditorNodes {
    #[node("Margin/VBox/Toolbar/SchemeSelect")]
    pub scheme_select: GodotNodeHandle,
    #[node("Margin/VBox/InfoBox/NameRow/NameEdit")]
    pub name_edit: GodotNodeHandle,
    #[node("Margin/VBox/InfoBox/DescRow/DescEdit")]
    pub desc_edit: GodotNodeHandle,
    #[node("Margin/VBox/Split/LeftPanel/LeftVBox/Tabs/参数/ParamsScroll/ParamsBox")]
    pub params_box: GodotNodeHandle,
    #[node("Margin/VBox/Split/LeftPanel/LeftVBox/Tabs/数据源/SourcesScroll/SourcesBox")]
    pub sources_box: GodotNodeHandle,
    #[node("Margin/VBox/Split/LeftPanel/LeftVBox/Tabs/数据源/ColsTitleRow/ColsTitle")]
    pub cols_title: GodotNodeHandle,
    #[node("Margin/VBox/Split/LeftPanel/LeftVBox/Tabs/数据源/ColsScroll/ColsBox")]
    pub cols_box: GodotNodeHandle,
    #[node("Margin/VBox/Split/LeftPanel/LeftVBox/Tabs/派生/DerivedScroll/DerivedBox")]
    pub derived_box: GodotNodeHandle,
    #[node("Margin/VBox/Split/LeftPanel/LeftVBox/Tabs/派生/DetailScroll/DetailVBox/DetailTitle")]
    pub derived_detail_title: GodotNodeHandle,
    #[node(
        "Margin/VBox/Split/LeftPanel/LeftVBox/Tabs/派生/DetailScroll/DetailVBox/IdRow/DerivedIdEdit"
    )]
    pub derived_id_edit: GodotNodeHandle,
    #[node(
        "Margin/VBox/Split/LeftPanel/LeftVBox/Tabs/派生/DetailScroll/DetailVBox/FromRow/FromSelect"
    )]
    pub derived_from_select: GodotNodeHandle,
    #[node(
        "Margin/VBox/Split/LeftPanel/LeftVBox/Tabs/派生/DetailScroll/DetailVBox/MergeRow/MergeEdit"
    )]
    pub derived_on_edit: GodotNodeHandle,
    #[node(
        "Margin/VBox/Split/LeftPanel/LeftVBox/Tabs/派生/DetailScroll/DetailVBox/WhenRow/WhenEdit"
    )]
    pub derived_when_edit: GodotNodeHandle,
    #[node(
        "Margin/VBox/Split/LeftPanel/LeftVBox/Tabs/派生/DetailScroll/DetailVBox/GroupRow/GroupEdit"
    )]
    pub derived_group_edit: GodotNodeHandle,
    #[node("Margin/VBox/Split/LeftPanel/LeftVBox/Tabs/派生/DetailScroll/DetailVBox/MapBox")]
    pub derived_map_box: GodotNodeHandle,
    #[node("Margin/VBox/Split/LeftPanel/LeftVBox/Tabs/派生/DetailScroll/DetailVBox/FieldBox")]
    pub derived_field_box: GodotNodeHandle,
    #[node("Margin/VBox/Split/LogPanel/Log")]
    pub log: GodotNodeHandle,
}

/// 解析到的编辑器节点句柄。
#[derive(Resource, Default)]
pub struct EditorRefs {
    pub nodes: Option<EditorNodes>,
}

/// 编辑器文档状态（Send）。
#[derive(Resource, Default)]
pub struct EditorDoc {
    /// 当前方案目标路径（未保存的新方案为拟创建路径）。
    pub path: Option<String>,
    /// 文件基名（不含 .json），即页面名称编辑框的初值。
    pub base: Option<String>,
    /// 内存中的方案 JSON（含顶层 schema_version/scheme）。
    pub raw: Option<serde_json::Value>,
    /// 数据源页签中当前选中编辑其列映射的数据源序号。
    pub selected_source: i32,
    /// 派生页签中当前选中查看详情（分组/映射/字段）的派生序号。
    pub selected_derived: i32,
}

/// 动态参数行控件（Gd 非 Send → NonSend）。
#[derive(Default)]
pub struct ParamRows {
    /// 参数名输入框（index 与渲染行一一对应）。
    pub name_edits: Vec<Gd<LineEdit>>,
    /// 参数类型下拉。
    pub type_buttons: Vec<Gd<OptionButton>>,
    /// 参数值输入框。
    pub value_edits: Vec<Gd<LineEdit>>,
    /// 每行删除按钮：(参数序号, 按钮)。
    pub remove_buttons: Vec<(i32, Gd<Button>)>,
    /// 已连接删除按钮数。
    pub connected: usize,
    /// 已连接值输入框数（text_changed）。
    pub value_connected: usize,
    /// 已连接类型下拉数（item_selected）。
    pub type_connected: usize,
    /// 类型是否被用户手动锁定（锁定后不再随值自动推断）。
    pub type_locked: Vec<bool>,
}

impl ParamRows {
    /// 释放全部动态行引用（页面切换/重建时调用）。
    pub fn clear(&mut self) {
        self.name_edits.clear();
        self.type_buttons.clear();
        self.value_edits.clear();
        self.remove_buttons.clear();
        self.connected = 0;
        self.value_connected = 0;
        self.type_connected = 0;
        self.type_locked.clear();
    }
}

/// GUI 可编辑的参数类型（与引擎 ColumnType 子集对齐，另见 parse_value）。
const PARAM_TYPES: [(&str, &str); 4] = [
    ("text", "文本"),
    ("integer", "整数"),
    ("decimal", "小数"),
    ("boolean", "布尔"),
];

/// 新参数默认类型：整数（多数参数是数量/上限类）。
const DEFAULT_TYPE: &str = "integer";

fn type_item_index(ty: &str) -> i32 {
    PARAM_TYPES
        .iter()
        .position(|(key, _)| *key == ty)
        .map_or(0, |i| i as i32)
}

fn type_key(item: i32) -> &'static str {
    PARAM_TYPES
        .get(item as usize)
        .map_or("text", |(key, _)| *key)
}

/// 依据值推断类型：整数 → integer；小数 → decimal；是/否/true/false → boolean；
/// 其余非空文本 → text；空白 → None（不改动当前选择）。
fn infer_type(value: &str) -> Option<&'static str> {
    let t = value.trim();
    if t.is_empty() {
        return None;
    }
    if matches!(t, "true" | "false" | "是" | "否") {
        return Some("boolean");
    }
    if t.parse::<i64>().is_ok() {
        return Some("integer");
    }
    if t.parse::<f64>().is_ok() {
        return Some("decimal");
    }
    Some("text")
}

/// 名称 → 文件基名：去首尾空白、去 .json 后缀与末尾点。
fn normalize_base(name: &str) -> String {
    let t = name.trim().to_string();
    let t = t.strip_suffix(".json").unwrap_or(&t).to_string();
    t.trim_end_matches('.').to_string()
}

// ---------- 初始化 ----------

pub fn resolve_editor(
    roots: Query<&GodotNodeHandle, With<EditorRoot>>,
    mut refs: ResMut<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut rows: bevy::prelude::NonSendMut<ParamRows>,
    mut src_rows: bevy::prelude::NonSendMut<SrcRows>,
    mut col_rows: bevy::prelude::NonSendMut<SrcCols>,
    mut dlist: bevy::prelude::NonSendMut<DvList>,
    mut ddetail: bevy::prelude::NonSendMut<DvDetail>,
    mut godot: GodotAccess,
) {
    if refs.nodes.is_some() {
        return;
    }
    for handle in &roots {
        let Some(control) = godot.try_get::<Control>(*handle) else {
            continue; // 场景尚未实例化完成
        };
        match EditorNodes::from_node(control) {
            Ok(nodes) => {
                info!("编辑器 NodeTreeView 解析成功");
                refs.nodes = Some(nodes);
                let Some(resolved) = &refs.nodes else {
                    return;
                };
                populate_scheme_select(resolved, &mut godot, Some("复刻"));
                load_current(
                    resolved,
                    &mut doc,
                    &mut rows,
                    &mut src_rows,
                    &mut col_rows,
                    &mut dlist,
                    &mut ddetail,
                    &mut godot,
                );
                return;
            }
            Err(_) => continue,
        }
    }
}

/// 动态控件的 typed 连接（每帧补连新出现的行控件）：
/// 值输入框 text_changed → 自动推断类型；类型下拉 item_selected → 锁定；删除按钮 pressed。
pub fn connect_param_signals(
    mut rows: bevy::prelude::NonSendMut<ParamRows>,
    value_signals: GodotSignals<ParamValueEdited>,
    type_signals: GodotSignals<ParamTypeChosen>,
    remove_signals: GodotSignals<ParamRemoveClicked>,
) {
    while rows.value_connected < rows.value_edits.len() {
        let index = rows.value_connected as i32;
        let handle =
            GodotNodeHandle::from_instance_id(rows.value_edits[rows.value_connected].instance_id());
        value_signals.connect(handle, "text_changed", None, move |_a, _n, _e| {
            Some(ParamValueEdited { index })
        });
        rows.value_connected += 1;
    }
    while rows.type_connected < rows.type_buttons.len() {
        let index = rows.type_connected as i32;
        let handle =
            GodotNodeHandle::from_instance_id(rows.type_buttons[rows.type_connected].instance_id());
        type_signals.connect(handle, "item_selected", None, move |_a, _n, _e| {
            Some(ParamTypeChosen { index })
        });
        rows.type_connected += 1;
    }
    while rows.connected < rows.remove_buttons.len() {
        let (index, button) = rows.remove_buttons[rows.connected].clone();
        let handle = GodotNodeHandle::from_instance_id(button.instance_id());
        remove_signals.connect(handle, "pressed", None, move |_a, _n, _e| {
            Some(ParamRemoveClicked { index })
        });
        rows.connected += 1;
    }
}

fn populate_scheme_select(nodes: &EditorNodes, godot: &mut GodotAccess, prefer: Option<&str>) {
    let names = schemes::scheme_names();
    let mut select = godot.get::<OptionButton>(nodes.scheme_select);
    select.clear();
    for name in &names {
        select.add_item(name);
    }
    if names.is_empty() {
        return;
    }
    let preferred = names
        .iter()
        .position(|n| prefer.map(|p| n.contains(p)).unwrap_or(false))
        .unwrap_or(0);
    select.select(preferred as i32);
}

// ---------- 载入 / 渲染 ----------

fn file_base_of(name: &str) -> String {
    normalize_base(name)
}

fn load_current(
    nodes: &EditorNodes,
    doc: &mut EditorDoc,
    rows: &mut ParamRows,
    src_rows: &mut SrcRows,
    col_rows: &mut SrcCols,
    dlist: &mut DvList,
    ddetail: &mut DvDetail,
    godot: &mut GodotAccess,
) {
    let file_name = godot
        .get::<OptionButton>(nodes.scheme_select)
        .get_text()
        .to_string();
    if file_name.is_empty() {
        log_warn(
            nodes,
            godot,
            "方案列表为空，请先新建方案或把 .json 放入用户方案目录",
        );
        return;
    }
    let path = schemes::scheme_path(&file_name);
    let raw_text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            log_err(nodes, godot, &format!("读取失败：{e}"));
            return;
        }
    };
    let raw: serde_json::Value = match serde_json::from_str(&raw_text) {
        Ok(v) => v,
        Err(e) => {
            log_err(nodes, godot, &format!("JSON 解析失败：{e}"));
            return;
        }
    };
    let base = file_base_of(&file_name);
    let desc = raw
        .get("scheme")
        .and_then(|s| s.get("description"))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    doc.path = Some(path);
    doc.base = Some(base.clone());
    doc.raw = Some(raw);

    godot.get::<LineEdit>(nodes.name_edit).set_text(&base);
    godot.get::<LineEdit>(nodes.desc_edit).set_text(&desc);
    render_param_rows(nodes, doc, rows, godot);
    doc.selected_source = 0;
    sources::render_all(nodes, doc, src_rows, col_rows, godot);
    doc.selected_derived = 0;
    derived::render_all(nodes, doc, dlist, ddetail, godot);
    log_ok(
        nodes,
        godot,
        &format!("已载入：{base}（按名称/参数编辑后保存）"),
    );
}

/// 依据 doc.raw 重建参数行（先释放 ParamsBox 旧内容）。
fn render_param_rows(
    nodes: &EditorNodes,
    doc: &EditorDoc,
    rows: &mut ParamRows,
    godot: &mut GodotAccess,
) {
    rows.clear();
    let params = doc
        .raw
        .as_ref()
        .and_then(|r| r.get("scheme"))
        .and_then(|s| s.get("parameters"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut box_node = godot.get::<VBoxContainer>(nodes.params_box);
    let child_count = box_node.get_child_count();
    for i in (0..child_count).rev() {
        if let Some(mut child) = box_node.get_child(i) {
            child.queue_free();
        }
    }

    if params.is_empty() {
        let mut hint = Label::new_alloc();
        hint.set_name("ParamEmptyHint");
        hint.set_text("暂无参数：点上方「+ 新增参数」添加（类型默认整数，输入值可自动识别）");
        hint.set_modulate(godot::builtin::Color::from_rgb(0.6, 0.6, 0.6));
        box_node.add_child(&hint);
        return;
    }

    for (i, p) in params.iter().enumerate() {
        let pname = p.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let ptype = p
            .get("value_type")
            .and_then(|v| v.as_str())
            .unwrap_or("text");
        let value = p.get("value").map(json_display).unwrap_or_default();

        let mut row = HBoxContainer::new_alloc();
        row.set_name(&format!("ParamRow{i}"));
        row.add_theme_constant_override("separation", 6);

        // 参数名（可编辑）
        let mut name_edit = LineEdit::new_alloc();
        name_edit.set_text(pname);
        name_edit.set_placeholder("参数名");
        name_edit.set_custom_minimum_size(Vector2::new(160.0, 0.0));
        name_edit.set_tooltip_text("参数名（引擎表达式引用标识，如 {请假封顶}）");
        row.add_child(&name_edit);

        // 类型下拉
        let mut type_btn = OptionButton::new_alloc();
        type_btn.set_tooltip_text("参数值类型");
        for (_, label) in PARAM_TYPES {
            type_btn.add_item(label);
        }
        type_btn.select(type_item_index(ptype));
        row.add_child(&type_btn);

        // 值（可编辑，扩展占满剩余宽度；输入变化自动识别类型，见 on_value_edited）
        let mut value_edit = LineEdit::new_alloc();
        value_edit.set_text(&value);
        value_edit.set_placeholder("参数值");
        value_edit.set_h_size_flags(SizeFlags::EXPAND_FILL);
        row.add_child(&value_edit);

        // 删除按钮
        let mut remove = Button::new_alloc();
        remove.set_name(&format!("DelParam{i}"));
        remove.set_text("删除");
        row.add_child(&remove);

        box_node.add_child(&row);
        rows.name_edits.push(name_edit);
        rows.type_buttons.push(type_btn);
        rows.value_edits.push(value_edit);
        rows.remove_buttons.push((i as i32, remove));
        rows.type_locked.push(false);
    }
}

// ---------- 事件处理 ----------

pub fn on_load_clicked(
    _trigger: On<EditorLoadClicked>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut rows: bevy::prelude::NonSendMut<ParamRows>,
    mut src_rows: bevy::prelude::NonSendMut<SrcRows>,
    mut col_rows: bevy::prelude::NonSendMut<SrcCols>,
    mut dlist: bevy::prelude::NonSendMut<DvList>,
    mut ddetail: bevy::prelude::NonSendMut<DvDetail>,
    mut godot: GodotAccess,
) {
    if let Some(nodes) = &refs.nodes {
        load_current(
            nodes,
            &mut doc,
            &mut rows,
            &mut src_rows,
            &mut col_rows,
            &mut dlist,
            &mut ddetail,
            &mut godot,
        );
    }
}

pub fn on_scheme_picked(
    _trigger: On<EditorSchemePicked>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut rows: bevy::prelude::NonSendMut<ParamRows>,
    mut src_rows: bevy::prelude::NonSendMut<SrcRows>,
    mut col_rows: bevy::prelude::NonSendMut<SrcCols>,
    mut dlist: bevy::prelude::NonSendMut<DvList>,
    mut ddetail: bevy::prelude::NonSendMut<DvDetail>,
    mut godot: GodotAccess,
) {
    if let Some(nodes) = &refs.nodes {
        load_current(
            nodes,
            &mut doc,
            &mut rows,
            &mut src_rows,
            &mut col_rows,
            &mut dlist,
            &mut ddetail,
            &mut godot,
        );
    }
}

pub fn on_new_scheme_clicked(
    _trigger: On<EditorNewSchemeClicked>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut rows: bevy::prelude::NonSendMut<ParamRows>,
    mut src_rows: bevy::prelude::NonSendMut<SrcRows>,
    mut col_rows: bevy::prelude::NonSendMut<SrcCols>,
    mut dlist: bevy::prelude::NonSendMut<DvList>,
    mut ddetail: bevy::prelude::NonSendMut<DvDetail>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = &refs.nodes else { return };
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let base = format!("未命名方案-{stamp}");
    let raw = serde_json::json!({
        "schema_version": 1,
        "scheme": {
            "name": base,
            "description": "",
            "entity": { "key_field": "姓名", "duplicate": "report" },
            "parameters": [],
            "sources": [],
            "derived": [],
            "aggregates": [],
            "groups": [],
            "outputs": []
        }
    });
    doc.path = Some(schemes::scheme_path(&format!("{base}.json")));
    doc.base = Some(base.clone());
    doc.selected_source = 0;
    doc.raw = Some(raw);

    godot.get::<LineEdit>(nodes.name_edit).set_text(&base);
    godot.get::<LineEdit>(nodes.desc_edit).set_text("");
    render_param_rows(nodes, &doc, &mut rows, &mut godot);
    sources::render_all(nodes, &doc, &mut src_rows, &mut col_rows, &mut godot);
    doc.selected_derived = 0;
    derived::render_all(nodes, &doc, &mut dlist, &mut ddetail, &mut godot);
    log_ok(
        nodes,
        &mut godot,
        "已新建（未保存）：填写名称/描述/参数/数据源后点「保存」落盘",
    );
}

pub fn on_add_param_clicked(
    _trigger: On<EditorAddParamClicked>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut rows: bevy::prelude::NonSendMut<ParamRows>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = &refs.nodes else { return };
    // 先把当前行内未保存的编辑提交回 raw，避免下面重建行时丢失。
    if let Err(err) = commit_rows_to_raw(&mut doc, &rows) {
        log_err(nodes, &mut godot, &format!("新增参数中止：{err}"));
        return;
    }
    let Some(raw) = &mut doc.raw else {
        log_err(nodes, &mut godot, "尚未载入方案");
        return;
    };
    let Some(params) = raw["scheme"]["parameters"].as_array_mut() else {
        log_err(nodes, &mut godot, "方案缺少 parameters 数组");
        return;
    };
    let idx = params.len();
    params.push(serde_json::json!({
        "name": format!("参数{idx}"),
        "value_type": DEFAULT_TYPE,
        "value": ""
    }));
    render_param_rows(nodes, &doc, &mut rows, &mut godot);
    log(
        nodes,
        &mut godot,
        &format!(
            "已新增参数（共 {} 个，默认整数，填值后自动识别类型）",
            idx + 1
        ),
    );
}

pub fn on_remove_param(
    trigger: On<ParamRemoveClicked>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut rows: bevy::prelude::NonSendMut<ParamRows>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = &refs.nodes else { return };
    let idx = trigger.event().index;
    // 先把当前行内未保存的编辑提交回 raw，避免下面重建行时丢失。
    if let Err(err) = commit_rows_to_raw(&mut doc, &rows) {
        log_err(nodes, &mut godot, &format!("删除参数中止：{err}"));
        return;
    }
    let Some(raw) = &mut doc.raw else { return };
    let Some(params) = raw["scheme"]["parameters"].as_array_mut() else {
        return;
    };
    if idx < 0 || idx as usize >= params.len() {
        return;
    }
    let removed_name = params[idx as usize]
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("?")
        .to_string();
    params.remove(idx as usize);
    render_param_rows(nodes, &doc, &mut rows, &mut godot);
    log(
        nodes,
        &mut godot,
        &format!("已删除参数 [{idx}]：{removed_name}"),
    );
}

pub fn on_value_edited(trigger: On<ParamValueEdited>, rows: bevy::prelude::NonSendMut<ParamRows>) {
    let index = trigger.event().index as usize;
    if index >= rows.value_edits.len() || index >= rows.type_buttons.len() {
        return;
    }
    // 类型被手动锁定后不再自动推断。
    if rows.type_locked.get(index).copied().unwrap_or(true) {
        return;
    }
    let value = rows.value_edits[index].get_text();
    let Some(ty) = infer_type(&value.to_string()) else {
        return; // 空白值不改动
    };
    let item = type_item_index(ty);
    if rows.type_buttons[index].get_selected() == item {
        return;
    }
    rows.type_buttons[index].clone().select(item);
}

pub fn on_type_chosen(
    trigger: On<ParamTypeChosen>,
    mut rows: bevy::prelude::NonSendMut<ParamRows>,
) {
    let index = trigger.event().index as usize;
    // 用户手动选择类型后锁定该行，不再随值自动推断。
    if index < rows.type_locked.len() {
        rows.type_locked[index] = true;
    }
}

pub fn on_validate_clicked(
    _trigger: On<EditorValidateClicked>,
    refs: Res<EditorRefs>,
    doc: Res<EditorDoc>,
    rows: bevy::prelude::NonSend<ParamRows>,
    src_rows: bevy::prelude::NonSend<SrcRows>,
    col_rows: bevy::prelude::NonSend<SrcCols>,
    _dlist: bevy::prelude::NonSend<DvList>,
    ddetail: bevy::prelude::NonSend<DvDetail>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = &refs.nodes else { return };
    // 校验内存中的当前编辑状态（未保存的内容也生效），不读磁盘旧文件。
    let Some(mut raw) = doc.raw.clone() else {
        log_err(nodes, &mut godot, "尚未载入方案");
        return;
    };
    let name_text = godot.get::<LineEdit>(nodes.name_edit).get_text();
    let new_base = normalize_base(&name_text.to_string());
    if new_base.is_empty() {
        log_err(nodes, &mut godot, "校验中止：方案名称不能为空");
        return;
    }
    raw["scheme"]["name"] = serde_json::Value::String(new_base);
    let desc = godot
        .get::<LineEdit>(nodes.desc_edit)
        .get_text()
        .to_string();
    raw["scheme"]["description"] = serde_json::Value::String(desc);
    // 参数行控件 → raw（与保存共用 write_params，含名称重复等检查）。
    if let Err(err) = write_params(&mut raw, &rows) {
        log_err(nodes, &mut godot, &format!("校验中止：{err}"));
        return;
    }
    // 数据源与列映射控件 → raw。
    if let Err(err) = sources::write_back(&mut raw, &src_rows, &col_rows, doc.selected_source) {
        log_err(nodes, &mut godot, &format!("校验中止：{err}"));
        return;
    }
    // 派生详情 → raw。
    if let Err(err) = derived::write_back(&mut raw, &ddetail, doc.selected_derived) {
        log_err(nodes, &mut godot, &format!("校验中止：{err}"));
        return;
    }
    let text = match serde_json::to_string(&raw) {
        Ok(t) => t,
        Err(e) => {
            log_err(nodes, &mut godot, &format!("校验中止：序列化失败 {e}"));
            return;
        }
    };
    let json = bridge::validate_scheme_json_text(&text);
    let out = bridge::render_result(&json);
    let kind = if out.ok { LogKind::Ok } else { LogKind::Err };
    log_kind(nodes, &mut godot, kind, &out.text);
}

pub fn on_save_clicked(
    _trigger: On<EditorSaveClicked>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    rows: bevy::prelude::NonSend<ParamRows>,
    src_rows: bevy::prelude::NonSend<SrcRows>,
    col_rows: bevy::prelude::NonSend<SrcCols>,
    _dlist: bevy::prelude::NonSend<DvList>,
    ddetail: bevy::prelude::NonSend<DvDetail>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = &refs.nodes else { return };
    let Some(mut raw) = doc.raw.clone() else {
        log_err(nodes, &mut godot, "尚未载入方案");
        return;
    };

    // 名称 / 描述（顶部编辑框）
    let name_text = godot.get::<LineEdit>(nodes.name_edit).get_text();
    let new_base = normalize_base(&name_text.to_string());
    if new_base.is_empty() {
        log_err(nodes, &mut godot, "保存失败：方案名称不能为空");
        return;
    }
    let desc = godot
        .get::<LineEdit>(nodes.desc_edit)
        .get_text()
        .to_string();
    let old_base = doc.base.clone();
    let old_path = doc.path.clone().unwrap_or_default();
    let renamed = old_base.as_deref() != Some(new_base.as_str());
    // 新建草稿：目标路径尚无文件（保存即首次落盘）。
    let is_first_save = !old_path.is_empty() && !std::path::Path::new(&old_path).exists();

    // 内存 JSON 更新（先于序列化）：描述、参数行、以及首次/改名时的内部 name。
    raw["scheme"]["description"] = serde_json::Value::String(desc);
    if renamed || is_first_save {
        raw["scheme"]["name"] = serde_json::Value::String(new_base.clone());
    }
    if let Err(err) = write_params(&mut raw, &rows) {
        log_err(nodes, &mut godot, &format!("保存失败：{err}"));
        return;
    }
    // 数据源与列映射控件 → raw。
    if let Err(err) = sources::write_back(&mut raw, &src_rows, &col_rows, doc.selected_source) {
        log_err(nodes, &mut godot, &format!("保存失败：{err}"));
        return;
    }
    // 派生详情 → raw。
    if let Err(err) = derived::write_back(&mut raw, &ddetail, doc.selected_derived) {
        log_err(nodes, &mut godot, &format!("保存失败：{err}"));
        return;
    }

    let new_path = schemes::scheme_path(&format!("{new_base}.json"));
    let text = match serde_json::to_string_pretty(&raw) {
        Ok(t) => t,
        Err(e) => {
            log_err(nodes, &mut godot, &format!("序列化方案失败：{e}"));
            return;
        }
    };
    if let Err(e) = std::fs::write(&new_path, text) {
        log_err(nodes, &mut godot, &format!("保存失败：{e}"));
        return;
    }
    // 改名保存：移除旧文件（新建草稿的旧路径通常不存在，忽略错误）。
    if renamed && !old_path.is_empty() && old_path != new_path {
        let _ = std::fs::remove_file(&old_path);
    }

    let note = if renamed {
        format!("（文件已由 {} 重命名）", old_base.as_deref().unwrap_or("?"))
    } else {
        String::new()
    };
    doc.path = Some(new_path.clone());
    doc.base = Some(new_base.clone());
    doc.raw = Some(raw);
    // 刷新下拉并选中当前文件
    populate_scheme_select(nodes, &mut godot, Some(new_base.as_str()));
    log_ok(nodes, &mut godot, &format!("已保存：{new_path}{note}"));
}

/// 把参数行控件当前内容提交回 raw（新增/删除参数前调用，避免重建行丢编辑）。
fn commit_rows_to_raw(doc: &mut EditorDoc, rows: &ParamRows) -> Result<(), String> {
    let Some(raw) = doc.raw.as_mut() else {
        return Err("尚未载入方案".into());
    };
    write_params(raw, rows)
}

/// 从参数行控件收集 name/type/value，写回 raw["scheme"]["parameters"]。
fn write_params(raw: &mut serde_json::Value, rows: &ParamRows) -> Result<(), String> {
    let Some(scheme_node) = raw.get_mut("scheme") else {
        return Err("缺少 scheme".into());
    };
    let Some(params) = scheme_node
        .get_mut("parameters")
        .and_then(|p| p.as_array_mut())
    else {
        return Err("缺少 parameters 数组".into());
    };
    params.clear();
    let mut seen: Vec<String> = Vec::new();
    for i in 0..rows.name_edits.len() {
        let name = rows.name_edits[i].get_text().to_string();
        let name = name.trim();
        if name.is_empty() {
            return Err(format!("第 {} 行参数名不能为空", i + 1));
        }
        if seen.iter().any(|s| s == name) {
            return Err(format!("参数名重复：{name}"));
        }
        seen.push(name.to_string());
        let ty = type_key(rows.type_buttons[i].get_selected());
        let text = rows.value_edits[i].get_text().to_string();
        // 值留空允许（未填状态），存空文本；运行时若被公式引用会在校验期暴露。
        let value = if text.trim().is_empty() {
            serde_json::Value::String(String::new())
        } else {
            parse_value(ty, &text)
                .ok_or_else(|| format!("参数[{name}] 的值无法解析为{}：{text}", type_label(ty)))?
        };
        params.push(serde_json::json!({
            "name": name,
            "value_type": ty,
            "value": value
        }));
    }
    Ok(())
}

fn type_label(ty: &str) -> &'static str {
    PARAM_TYPES
        .iter()
        .find(|(key, _)| *key == ty)
        .map_or("文本", |(_, label)| *label)
}

fn parse_value(ty: &str, text: &str) -> Option<serde_json::Value> {
    let t = text.trim();
    match ty {
        "integer" => t.parse::<i64>().ok().map(serde_json::Value::from),
        "decimal" => t.parse::<f64>().ok().map(serde_json::Value::from),
        "boolean" => match t {
            "true" | "1" | "是" => Some(serde_json::Value::Bool(true)),
            "false" | "0" | "否" => Some(serde_json::Value::Bool(false)),
            _ => None,
        },
        _ => Some(serde_json::Value::String(t.to_string())),
    }
}

fn json_display(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn log(nodes: &EditorNodes, godot: &mut GodotAccess, line: &str) {
    log_kind(nodes, godot, LogKind::Plain, line);
}

fn log_ok(nodes: &EditorNodes, godot: &mut GodotAccess, line: &str) {
    log_kind(nodes, godot, LogKind::Ok, line);
}

fn log_warn(nodes: &EditorNodes, godot: &mut GodotAccess, line: &str) {
    log_kind(nodes, godot, LogKind::Warn, line);
}

fn log_err(nodes: &EditorNodes, godot: &mut GodotAccess, line: &str) {
    log_kind(nodes, godot, LogKind::Err, line);
}

fn log_kind(nodes: &EditorNodes, godot: &mut GodotAccess, kind: LogKind, line: &str) {
    info!("{line}");
    log_line(&mut godot.get::<RichTextLabel>(nodes.log), kind, line);
}
