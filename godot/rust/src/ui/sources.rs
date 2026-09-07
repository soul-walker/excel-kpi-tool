//! 数据源页签：数据源（Excel 台账）与列映射编辑。
//!
//! 数据模型（raw["scheme"]["sources"]）：
//! ```json
//! [ { "id", "file", "sheet", "header_row", "parse",
//!     "columns": [ { "header"|"index", "alias", "value_type", "key", "required"?, "bits"? } ] } ]
//! ```
//! `file` 为相对月份目录的文件名（旧版 `{monthDir}/` 前缀展示/保存时自动去掉）。
//! UI：每源渲染两行卡片（源信息行 + 文件行）；选中源后在下方列映射区编辑列
//! （表头 / 别名 / 类型 / 是否主键）。`parse`/`header_row`/`bits` 暂无图形编辑，原值保留。
//! 增删/切换（数据源、列映射选中）都会重建页面，故这些动作前先把行控件与
//! 当前列网格的未保存编辑写回 raw（`flush_open`），编辑不会因重建丢失。
#![allow(clippy::too_many_arguments)]

use bevy::ecs::observer::On;
use bevy::prelude::{Res, ResMut};
use godot::builtin::Vector2;
use godot::classes::control::SizeFlags;
use godot::classes::{
    Button, CheckButton, GridContainer, Label, LineEdit, OptionButton, RichTextLabel, VBoxContainer,
};
use godot::obj::NewAlloc;
use godot::prelude::Gd;
use godot_bevy::prelude::{GodotAccess, GodotNodeHandle, GodotSignals};
use tracing::info;

use super::aggregates::{self, AggregateDetail as AggDetail};
use super::derived::{self, DerivedDetail};
use super::editor::{EditorDoc, EditorNodes, EditorRefs};
use super::events::{
    AddColClicked, AddSourceClicked, ColRemoveClicked, NameFieldFocusLost, SourceColsClicked,
    SourceRemoveClicked,
};
use super::outputs;
use super::outputs::OutputDetail as OutDetail;
use super::refs;
use super::view::{LogKind, log_line};

/// 列可编辑的值类型（比参数多“日期”）。
const COL_TYPES: [(&str, &str); 5] = [
    ("text", "文本"),
    ("integer", "整数"),
    ("decimal", "小数"),
    ("boolean", "布尔"),
    ("date", "日期"),
];

fn col_type_index(ty: &str) -> i32 {
    COL_TYPES
        .iter()
        .position(|(key, _)| *key == ty)
        .map_or(0, |i| i as i32)
}

fn col_type_key(item: i32) -> &'static str {
    COL_TYPES.get(item as usize).map_or("text", |(key, _)| *key)
}

/// 数据源卡片行控件（Gd 非 Send → NonSend）。
#[derive(Default)]
pub struct SourceRows {
    pub id_edits: Vec<Gd<LineEdit>>,
    pub file_edits: Vec<Gd<LineEdit>>,
    pub sheet_edits: Vec<Gd<LineEdit>>,
    /// “列映射”按钮：(源序号, 按钮)。
    pub col_buttons: Vec<(i32, Gd<Button>)>,
    /// 删除按钮：(源序号, 按钮)。
    pub del_buttons: Vec<(i32, Gd<Button>)>,
    pub connected_col: usize,
    pub connected_del: usize,
    /// 已连接“源 id”输入框失焦数（改名即同步）。
    pub id_focus_connected: usize,
}

impl SourceRows {
    pub fn clear(&mut self) {
        self.id_edits.clear();
        self.file_edits.clear();
        self.sheet_edits.clear();
        self.col_buttons.clear();
        self.del_buttons.clear();
        self.connected_col = 0;
        self.connected_del = 0;
        self.id_focus_connected = 0;
    }
}

/// 当前选中数据源的列行控件（Gd 非 Send → NonSend）。
#[derive(Default)]
pub struct ColRows {
    pub header_edits: Vec<Gd<LineEdit>>,
    pub alias_edits: Vec<Gd<LineEdit>>,
    pub type_buttons: Vec<Gd<OptionButton>>,
    pub key_checks: Vec<Gd<CheckButton>>,
    pub del_buttons: Vec<(i32, Gd<Button>)>,
    pub connected: usize,
    /// 列网格当前正在展示的数据源序号（None = 无源/尚未渲染）。
    /// 结构性操作前据此把未保存编辑写回对应源。
    pub rendered_for: Option<i32>,
    /// 已连接“列 alias”输入框失焦数（改名即同步）。
    pub alias_focus_connected: usize,
}

impl ColRows {
    pub fn clear(&mut self) {
        self.header_edits.clear();
        self.alias_edits.clear();
        self.type_buttons.clear();
        self.key_checks.clear();
        self.del_buttons.clear();
        self.connected = 0;
        self.rendered_for = None;
        self.alias_focus_connected = 0;
    }
}

// ---------- 渲染 ----------

/// 清空容器全部子节点（重建前释放）。
fn clear_children(container: &mut VBoxContainer) {
    let n = container.get_child_count();
    for i in (0..n).rev() {
        if let Some(mut child) = container.get_child(i) {
            child.queue_free();
        }
    }
}

/// 重建数据源列表与列映射区（载入/增删/切换时调用）。
pub fn render_all(
    nodes: &EditorNodes,
    doc: &EditorDoc,
    rows: &mut SourceRows,
    cols: &mut ColRows,
    godot: &mut GodotAccess,
) {
    render_sources(nodes, doc, rows, godot);
    render_cols(nodes, doc, cols, godot);
}

/// 表头灰色样式。
fn head_label(
    grid: &mut GridContainer,
    text: &str,
    min_x: f32,
    expand: bool,
    gray: godot::builtin::Color,
) {
    let mut label = Label::new_alloc();
    label.set_text(text);
    label.set_modulate(gray);
    if min_x > 0.0 {
        label.set_custom_minimum_size(Vector2::new(min_x, 0.0));
    }
    if expand {
        label.set_h_size_flags(SizeFlags::EXPAND_FILL);
    }
    grid.add_child(&label);
}

fn render_sources(
    nodes: &EditorNodes,
    doc: &EditorDoc,
    rows: &mut SourceRows,
    godot: &mut GodotAccess,
) {
    rows.clear();
    let srcs = doc
        .raw
        .as_ref()
        .and_then(|r| r.get("scheme"))
        .and_then(|s| s.get("sources"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut box_node = godot.get::<VBoxContainer>(nodes.sources_box);
    clear_children(&mut box_node);
    if srcs.is_empty() {
        let mut hint = Label::new_alloc();
        hint.set_name("SourceEmptyHint");
        hint.set_text("暂无数据源：点「+ 新增数据源」");
        hint.set_tooltip_text("新增数据源默认带 姓名（主键）列；文件名相对运行月份目录，可到下方「列映射」调整读取列。");
        hint.set_modulate(godot::builtin::Color::from_rgb(0.6, 0.6, 0.6));
        box_node.add_child(&hint);
        return;
    }

    let gray = godot::builtin::Color::from_rgb(0.62, 0.62, 0.62);
    let mut grid = GridContainer::new_alloc();
    grid.set_name("SourcesGrid");
    grid.set_columns(6);
    grid.add_theme_constant_override("h_separation", 6);
    grid.add_theme_constant_override("v_separation", 2);
    // 表头：源 / id / 文件 / sheet / 操作
    head_label(&mut grid, "源", 40.0, false, gray);
    head_label(&mut grid, "id", 130.0, false, gray);
    head_label(&mut grid, "文件（月份目录下）", 0.0, true, gray);
    head_label(&mut grid, "sheet", 110.0, false, gray);
    head_label(&mut grid, "列映射", 0.0, false, gray);
    head_label(&mut grid, "删除", 0.0, false, gray);

    for (i, s) in srcs.iter().enumerate() {
        let id = s.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let file = s.get("file").and_then(|v| v.as_str()).unwrap_or("");
        let sheet = s.get("sheet").and_then(|v| v.as_str()).unwrap_or("");

        // 列0：源序号
        let mut idx_label = Label::new_alloc();
        idx_label.set_text(&format!("源{}", i + 1));
        idx_label.set_custom_minimum_size(Vector2::new(40.0, 0.0));
        grid.add_child(&idx_label);

        // 列1：id
        let mut id_edit = LineEdit::new_alloc();
        id_edit.set_text(id);
        id_edit.set_placeholder("id");
        id_edit.set_custom_minimum_size(Vector2::new(130.0, 0.0));
        id_edit.set_tooltip_text("数据源标识（派生/聚合按 id 引用；可用中文，如 课时表）");
        grid.add_child(&id_edit);

        // 列2：相对月份目录的文件名（占满剩余宽度）
        let mut file_edit = LineEdit::new_alloc();
        file_edit.set_text(&display_file(file));
        file_edit.set_placeholder("课时.xlsx");
        file_edit.set_h_size_flags(SizeFlags::EXPAND_FILL);
        file_edit.set_tooltip_text(
            "相对运行台所选月份目录的文件名；旧版 {monthDir}/ 前缀保存时自动去掉",
        );
        grid.add_child(&file_edit);

        // 列3：sheet
        let mut sheet_edit = LineEdit::new_alloc();
        sheet_edit.set_text(sheet);
        sheet_edit.set_placeholder("Sheet1");
        sheet_edit.set_custom_minimum_size(Vector2::new(110.0, 0.0));
        grid.add_child(&sheet_edit);

        // 列4/5：列映射、删除
        let mut col_btn = Button::new_alloc();
        col_btn.set_text("列映射");
        grid.add_child(&col_btn);
        let mut del_btn = Button::new_alloc();
        del_btn.set_text("删除");
        grid.add_child(&del_btn);

        rows.id_edits.push(id_edit);
        rows.file_edits.push(file_edit);
        rows.sheet_edits.push(sheet_edit);
        rows.col_buttons.push((i as i32, col_btn));
        rows.del_buttons.push((i as i32, del_btn));
    }
    box_node.add_child(&grid);
}

fn render_cols(nodes: &EditorNodes, doc: &EditorDoc, cols: &mut ColRows, godot: &mut GodotAccess) {
    cols.clear();
    let srcs = doc
        .raw
        .as_ref()
        .and_then(|r| r.get("scheme"))
        .and_then(|s| s.get("sources"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let sel = (doc.selected_source as usize).min(srcs.len().saturating_sub(1));
    cols.rendered_for = if srcs.is_empty() {
        None
    } else {
        Some(sel as i32)
    };

    let mut title = godot.get::<Label>(nodes.cols_title);
    if let Some(s) = srcs.get(sel) {
        let sid = s.get("id").and_then(|v| v.as_str()).unwrap_or("?");
        title.set_text(&format!("列映射 · {}（源 {}）", sid, sel + 1));
    } else {
        title.set_text("列映射（请先添加数据源）");
    }

    let mut box_node = godot.get::<VBoxContainer>(nodes.cols_box);
    clear_children(&mut box_node);
    let Some(s) = srcs.get(sel) else {
        return;
    };
    let columns = s
        .get("columns")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    if columns.is_empty() {
        let mut hint = Label::new_alloc();
        hint.set_name("ColEmptyHint");
        hint.set_text("暂无列：点「+ 新增列」添加");
        hint.set_tooltip_text(
            "表头名 = Excel 列标题文字（定位用）；别名 = 表达式里的引用名，如 {数据源id}.{别名}。",
        );
        hint.set_modulate(godot::builtin::Color::from_rgb(0.6, 0.6, 0.6));
        box_node.add_child(&hint);
        return;
    }

    let gray = godot::builtin::Color::from_rgb(0.62, 0.62, 0.62);
    let mut grid = GridContainer::new_alloc();
    grid.set_name("ColsGrid");
    grid.set_columns(5);
    grid.add_theme_constant_override("h_separation", 6);
    grid.add_theme_constant_override("v_separation", 2);
    // 表头：表头名 / 别名 / 类型 / 主键 / 操作
    head_label(&mut grid, "表头名（Excel 列标题）", 0.0, true, gray);
    head_label(&mut grid, "别名（表达式引用）", 150.0, false, gray);
    head_label(&mut grid, "类型", 0.0, false, gray);
    head_label(&mut grid, "主键", 0.0, false, gray);
    head_label(&mut grid, "删除", 0.0, false, gray);

    for (j, c) in columns.iter().enumerate() {
        let header = c.get("header").and_then(|v| v.as_str()).unwrap_or("");
        let alias = c.get("alias").and_then(|v| v.as_str()).unwrap_or("");
        let ty = c
            .get("value_type")
            .and_then(|v| v.as_str())
            .unwrap_or("text");
        let key = c.get("key").and_then(|v| v.as_bool()).unwrap_or(false);

        // 列0：表头名
        let mut header_edit = LineEdit::new_alloc();
        header_edit.set_text(header);
        header_edit.set_placeholder("表头名");
        header_edit.set_h_size_flags(SizeFlags::EXPAND_FILL);
        header_edit.set_tooltip_text("Excel 列标题文字（留空则用别名按列序定位）");
        grid.add_child(&header_edit);

        // 列1：别名
        let mut alias_edit = LineEdit::new_alloc();
        alias_edit.set_text(alias);
        alias_edit.set_placeholder("别名");
        alias_edit.set_custom_minimum_size(Vector2::new(150.0, 0.0));
        alias_edit.set_tooltip_text("引擎内引用名（表达式 / key 用），可中文");
        grid.add_child(&alias_edit);

        // 列2：类型
        let mut type_btn = OptionButton::new_alloc();
        for (_, label) in COL_TYPES {
            type_btn.add_item(label);
        }
        type_btn.select(col_type_index(ty));
        grid.add_child(&type_btn);

        // 列3：主键
        let mut key_check = CheckButton::new_alloc();
        key_check.set_text("主键");
        key_check.set_pressed(key);
        grid.add_child(&key_check);

        // 列4：删除
        let mut del_btn = Button::new_alloc();
        del_btn.set_text("删除");
        grid.add_child(&del_btn);

        cols.header_edits.push(header_edit);
        cols.alias_edits.push(alias_edit);
        cols.type_buttons.push(type_btn);
        cols.key_checks.push(key_check);
        cols.del_buttons.push((j as i32, del_btn));
    }
    box_node.add_child(&grid);
}

// ---------- 动态信号连接 ----------

/// 每帧补连新渲染的按钮。
pub fn connect_sources_signals(
    mut rows: bevy::prelude::NonSendMut<SourceRows>,
    mut cols: bevy::prelude::NonSendMut<ColRows>,
    source_del: GodotSignals<SourceRemoveClicked>,
    source_cols: GodotSignals<SourceColsClicked>,
    col_del: GodotSignals<ColRemoveClicked>,
    name_focus: GodotSignals<NameFieldFocusLost>,
) {
    while rows.connected_col < rows.col_buttons.len() {
        let (index, button) = rows.col_buttons[rows.connected_col].clone();
        let handle = GodotNodeHandle::from_instance_id(button.instance_id());
        source_cols.connect(handle, "pressed", None, move |_a, _n, _e| {
            Some(SourceColsClicked { index })
        });
        rows.connected_col += 1;
    }
    while rows.connected_del < rows.del_buttons.len() {
        let (index, button) = rows.del_buttons[rows.connected_del].clone();
        let handle = GodotNodeHandle::from_instance_id(button.instance_id());
        source_del.connect(handle, "pressed", None, move |_a, _n, _e| {
            Some(SourceRemoveClicked { index })
        });
        rows.connected_del += 1;
    }
    while cols.connected < cols.del_buttons.len() {
        let (index, button) = cols.del_buttons[cols.connected].clone();
        let handle = GodotNodeHandle::from_instance_id(button.instance_id());
        col_del.connect(handle, "pressed", None, move |_a, _n, _e| {
            Some(ColRemoveClicked { index })
        });
        cols.connected += 1;
    }
    // 源 id / 列 alias 输入框失焦 → 改名即同步。
    while rows.id_focus_connected < rows.id_edits.len() {
        let handle =
            GodotNodeHandle::from_instance_id(rows.id_edits[rows.id_focus_connected].instance_id());
        name_focus.connect(handle, "focus_exited", None, move |_a, n, _e| {
            Some(NameFieldFocusLost { node: n })
        });
        rows.id_focus_connected += 1;
    }
    while cols.alias_focus_connected < cols.alias_edits.len() {
        let handle = GodotNodeHandle::from_instance_id(
            cols.alias_edits[cols.alias_focus_connected].instance_id(),
        );
        name_focus.connect(handle, "focus_exited", None, move |_a, n, _e| {
            Some(NameFieldFocusLost { node: n })
        });
        cols.alias_focus_connected += 1;
    }
}

// ---------- 增删（观察者） ----------

/// 增删/切换前的“落库”：把数据源行与当前列网格里尚未保存的编辑写回 raw。
/// 若控件与模型行数对不上（页面刚重建/未渲染），宁可不动模型。
fn flush_open(raw: &mut serde_json::Value, rows: &SourceRows, cols: &ColRows) {
    let Some(srcs) = raw
        .get_mut("scheme")
        .and_then(|s| s.get_mut("sources"))
        .and_then(|v| v.as_array_mut())
    else {
        return;
    };
    // 数据源行：id / 文件 / sheet 原样写回（不做校验；保存/校验时仍会查空）。
    if rows.id_edits.len() == srcs.len() {
        for (i, src) in srcs.iter_mut().enumerate() {
            let id = rows.id_edits[i].get_text().to_string();
            let file = rows.file_edits[i].get_text().to_string();
            let sheet = rows.sheet_edits[i].get_text().to_string();
            src["id"] = serde_json::Value::String(id.trim().to_string());
            src["file"] = serde_json::Value::String(normalize_file(&file));
            src["sheet"] = serde_json::Value::String(sheet.trim().to_string());
            // 旧版 label 字段已废弃：随保存一并移除。
            if let Some(obj) = src.as_object_mut() {
                obj.remove("label");
            }
        }
    }
    // 列映射：只写回当前正展示的那个源（其网格控件确实由它渲染而来）。
    let Some(idx) = cols.rendered_for else {
        return;
    };
    let Some(src) = srcs.get_mut(idx as usize) else {
        return;
    };
    let mut columns = Vec::new();
    for j in 0..cols.alias_edits.len() {
        let alias = cols.alias_edits[j].get_text().to_string();
        let header = cols.header_edits[j].get_text().to_string();
        let ty = col_type_key(cols.type_buttons[j].get_selected());
        let key = cols.key_checks[j].is_pressed();
        let mut col = serde_json::json!({ "alias": alias, "value_type": ty, "key": key });
        let header = header.trim();
        if header.is_empty() {
            col["header"] = serde_json::Value::Null;
        } else {
            col["header"] = serde_json::Value::String(header.to_string());
        }
        columns.push(col);
    }
    src["columns"] = serde_json::Value::Array(columns);
}

/// 增删/切换选中前调用：把控件里的未保存编辑先落回 raw，再改动模型并重建页面。
fn flush_open_doc(doc: &mut EditorDoc, rows: &SourceRows, cols: &ColRows) {
    let Some(raw) = doc.raw.as_mut() else {
        return;
    };
    flush_open(raw, rows, cols);
}

/// 源/派生/聚合三处“落库”后做改名传播：其它页面板未保存的编辑也要先写回，
/// 否则改名（源 id/列 alias/派生 id/视图列名/指标 id）同步不到它们。返回改名日志（空=无）。
fn flush_and_propagate(
    doc: &mut EditorDoc,
    rows: &SourceRows,
    cols: &ColRows,
    detail: &DerivedDetail,
    adetail: &AggDetail,
    odetail: &OutDetail,
) -> Vec<String> {
    let before = doc.raw.clone();
    derived::flush_open(doc, detail);
    aggregates::flush_open(doc, adetail);
    outputs::flush_open(doc, odetail);
    flush_open_doc(doc, rows, cols);
    match (before.as_ref(), doc.raw.as_mut()) {
        (Some(b), Some(a)) => refs::propagate(b, a),
        _ => Vec::new(),
    }
}

/// 源页面重建后，若传播过改名则重建派生/聚合/输出页，让引用类控件显示同步后的值。
fn log_and_sync_derived(
    nodes: &EditorNodes,
    doc: &EditorDoc,
    detail: &mut DerivedDetail,
    adetail: &mut AggDetail,
    odetail: &mut OutDetail,
    godot: &mut GodotAccess,
    renames: &[String],
) {
    for line in renames {
        log_plain(nodes, godot, line);
    }
    if !renames.is_empty() {
        derived::render_all(nodes, doc, detail, godot);
        aggregates::render_all(nodes, doc, adetail, godot);
        outputs::render_all(nodes, doc, odetail, godot);
    }
}

fn sources_mut(doc: &mut EditorDoc) -> Option<&mut Vec<serde_json::Value>> {
    doc.raw
        .as_mut()
        .and_then(|r| r.get_mut("scheme"))
        .and_then(|s| s.get_mut("sources"))
        .and_then(|v| v.as_array_mut())
}

fn nodes_of(refs: &EditorRefs) -> Option<&EditorNodes> {
    refs.nodes.as_ref()
}

pub fn on_add_source(
    _trigger: On<AddSourceClicked>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut rows: bevy::prelude::NonSendMut<SourceRows>,
    mut cols: bevy::prelude::NonSendMut<ColRows>,
    mut detail: bevy::prelude::NonSendMut<DerivedDetail>,
    mut adetail: bevy::prelude::NonSendMut<AggDetail>,
    mut odetail: bevy::prelude::NonSendMut<OutDetail>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = nodes_of(&refs) else { return };
    // 先把当前行内未保存的编辑提交回 raw，避免下面重建整页时丢失。
    let renames = flush_and_propagate(&mut doc, &rows, &cols, &detail, &adetail, &odetail);
    let n = {
        let Some(srcs) = sources_mut(&mut doc) else {
            log_err(nodes, &mut godot, "尚未载入方案");
            return;
        };
        srcs.push(serde_json::json!({
            "id": format!("源{}", srcs.len() + 1),
            "file": "新建台账.xlsx",
            "sheet": "Sheet1",
            "header_row": 1,
            "parse": {},
            "columns": [
                { "header": "姓名", "alias": "姓名", "value_type": "text", "key": true }
            ]
        }));
        srcs.len()
    };
    doc.selected_source = n as i32 - 1;
    render_all(nodes, &doc, &mut rows, &mut cols, &mut godot);
    log_and_sync_derived(
        nodes,
        &doc,
        &mut detail,
        &mut adetail,
        &mut odetail,
        &mut godot,
        &renames,
    );
    log_plain(
        nodes,
        &mut godot,
        &format!("已新增数据源（共 {n} 个，默认带 姓名/主键 列）"),
    );
}

pub fn on_remove_source(
    trigger: On<SourceRemoveClicked>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut rows: bevy::prelude::NonSendMut<SourceRows>,
    mut cols: bevy::prelude::NonSendMut<ColRows>,
    mut detail: bevy::prelude::NonSendMut<DerivedDetail>,
    mut adetail: bevy::prelude::NonSendMut<AggDetail>,
    mut odetail: bevy::prelude::NonSendMut<OutDetail>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = nodes_of(&refs) else { return };
    let idx = trigger.event().index;
    // 先把当前行内未保存的编辑提交回 raw，避免删除重建时丢失。
    let renames = flush_and_propagate(&mut doc, &rows, &cols, &detail, &adetail, &odetail);
    let removed_id = {
        let Some(srcs) = sources_mut(&mut doc) else {
            return;
        };
        if idx < 0 || idx as usize >= srcs.len() {
            return;
        }
        let rid = srcs[idx as usize]
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_string();
        srcs.remove(idx as usize);
        rid
    };
    let last = sources_of(&doc).map_or(0, |s| s.len() as i32 - 1);
    if doc.selected_source > last {
        doc.selected_source = last.max(0);
    }
    render_all(nodes, &doc, &mut rows, &mut cols, &mut godot);
    log_and_sync_derived(
        nodes,
        &doc,
        &mut detail,
        &mut adetail,
        &mut odetail,
        &mut godot,
        &renames,
    );
    log_plain(
        nodes,
        &mut godot,
        &format!("已删除数据源 [{idx}]：{removed_id}"),
    );
}

pub fn on_source_cols(
    trigger: On<SourceColsClicked>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut rows: bevy::prelude::NonSendMut<SourceRows>,
    mut cols: bevy::prelude::NonSendMut<ColRows>,
    mut detail: bevy::prelude::NonSendMut<DerivedDetail>,
    mut adetail: bevy::prelude::NonSendMut<AggDetail>,
    mut odetail: bevy::prelude::NonSendMut<OutDetail>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = nodes_of(&refs) else { return };
    // 切到另一个源前，先把当前源行/列网格的未保存编辑落回 raw。
    let renames = flush_and_propagate(&mut doc, &rows, &cols, &detail, &adetail, &odetail);
    doc.selected_source = trigger.event().index.max(0);
    render_all(nodes, &doc, &mut rows, &mut cols, &mut godot);
    log_and_sync_derived(
        nodes,
        &doc,
        &mut detail,
        &mut adetail,
        &mut odetail,
        &mut godot,
        &renames,
    );
    let sid = sources_of(&doc)
        .and_then(|s| s.get(doc.selected_source as usize))
        .and_then(|s| s.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    log_plain(nodes, &mut godot, &format!("编辑列映射：{sid}"));
}

pub fn on_add_col(
    _trigger: On<AddColClicked>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut rows: bevy::prelude::NonSendMut<SourceRows>,
    mut cols: bevy::prelude::NonSendMut<ColRows>,
    mut detail: bevy::prelude::NonSendMut<DerivedDetail>,
    mut adetail: bevy::prelude::NonSendMut<AggDetail>,
    mut odetail: bevy::prelude::NonSendMut<OutDetail>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = nodes_of(&refs) else { return };
    // 先把当前行内未保存的编辑提交回 raw，避免重建列网格时把已编辑列还原。
    let renames = flush_and_propagate(&mut doc, &rows, &cols, &detail, &adetail, &odetail);
    let result = (|| -> Result<usize, String> {
        let sel = doc.selected_source as usize;
        let Some(srcs) = sources_mut(&mut doc) else {
            return Err("尚未载入方案".to_string());
        };
        if sel >= srcs.len() {
            return Err("请先选择一个数据源".to_string());
        }
        let Some(cols_arr) = srcs[sel].get_mut("columns").and_then(|v| v.as_array_mut()) else {
            return Err("数据源缺少 columns 数组".to_string());
        };
        let n = cols_arr.len();
        cols_arr.push(serde_json::json!({
            "header": "", "alias": format!("列{}", n + 1), "value_type": "text", "key": false
        }));
        Ok(n + 1)
    })();
    match result {
        Ok(total) => {
            render_all(nodes, &doc, &mut rows, &mut cols, &mut godot);
            log_and_sync_derived(
                nodes,
                &doc,
                &mut detail,
                &mut adetail,
                &mut odetail,
                &mut godot,
                &renames,
            );
            log_plain(
                nodes,
                &mut godot,
                &format!("已给数据源新增列（共 {total} 列）"),
            );
        }
        Err(e) => log_err(nodes, &mut godot, &e),
    }
}

pub fn on_remove_col(
    trigger: On<ColRemoveClicked>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut rows: bevy::prelude::NonSendMut<SourceRows>,
    mut cols: bevy::prelude::NonSendMut<ColRows>,
    mut detail: bevy::prelude::NonSendMut<DerivedDetail>,
    mut adetail: bevy::prelude::NonSendMut<AggDetail>,
    mut odetail: bevy::prelude::NonSendMut<OutDetail>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = nodes_of(&refs) else { return };
    let idx = trigger.event().index;
    // 先把当前未保存的编辑提交回 raw，避免删列重建时把其它已编辑列还原。
    let renames = flush_and_propagate(&mut doc, &rows, &cols, &detail, &adetail, &odetail);
    let sel = doc.selected_source as usize;
    let removed = {
        let Some(srcs) = sources_mut(&mut doc) else {
            return;
        };
        let Some(cols_arr) = srcs
            .get_mut(sel)
            .and_then(|s| s.get_mut("columns"))
            .and_then(|v| v.as_array_mut())
        else {
            return;
        };
        if idx < 0 || idx as usize >= cols_arr.len() {
            return;
        }
        let removed = cols_arr[idx as usize]
            .get("alias")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_string();
        cols_arr.remove(idx as usize);
        removed
    };
    render_all(nodes, &doc, &mut rows, &mut cols, &mut godot);
    log_and_sync_derived(
        nodes,
        &doc,
        &mut detail,
        &mut adetail,
        &mut odetail,
        &mut godot,
        &renames,
    );
    log_plain(nodes, &mut godot, &format!("已删除列 [{idx}]：{removed}"));
}

// ---------- 写回（保存 / 校验前调用） ----------

/// 把数据源行控件内容写回 raw["scheme"]["sources"]；列映射只写回当前选中源。
pub fn write_back(
    raw: &mut serde_json::Value,
    rows: &SourceRows,
    cols: &ColRows,
    selected: i32,
) -> Result<(), String> {
    let Some(scheme_node) = raw.get_mut("scheme") else {
        return Err("缺少 scheme".into());
    };
    let Some(srcs) = scheme_node
        .get_mut("sources")
        .and_then(|s| s.as_array_mut())
    else {
        return Err("缺少 sources 数组".into());
    };
    if rows.id_edits.len() != srcs.len() {
        return Err("数据源行数与内容不一致，请重新载入后保存".into());
    }
    for (i, src) in srcs.iter_mut().enumerate() {
        let id = rows.id_edits[i].get_text().to_string();
        let file = rows.file_edits[i].get_text().to_string();
        let sheet = rows.sheet_edits[i].get_text().to_string();
        if id.trim().is_empty() {
            return Err(format!("第 {} 个数据源：id 不能为空", i + 1));
        }
        if file.trim().is_empty() {
            return Err(format!("数据源[{id}]：文件不能为空"));
        }
        if sheet.trim().is_empty() {
            return Err(format!("数据源[{id}]：sheet 名不能为空"));
        }
        src["id"] = serde_json::Value::String(id.trim().to_string());
        src["file"] = serde_json::Value::String(normalize_file(&file));
        src["sheet"] = serde_json::Value::String(sheet.trim().to_string());
        // 旧版 label 字段已废弃：保存时移除。
        if let Some(obj) = src.as_object_mut() {
            obj.remove("label");
        }
    }
    // 当前选中源的列映射写回
    if (selected as usize) < srcs.len() {
        let mut columns = Vec::new();
        for j in 0..cols.alias_edits.len() {
            let alias = cols.alias_edits[j].get_text().to_string();
            let alias = alias.trim().to_string();
            if alias.is_empty() {
                return Err(format!("列映射第 {} 行：别名（alias）不能为空", j + 1));
            }
            let header = cols.header_edits[j].get_text().to_string();
            let ty = col_type_key(cols.type_buttons[j].get_selected());
            let key = cols.key_checks[j].is_pressed();
            let mut col = serde_json::json!({
                "alias": alias,
                "value_type": ty,
                "key": key,
            });
            let header = header.trim();
            if header.is_empty() {
                col["header"] = serde_json::Value::Null;
            } else {
                col["header"] = serde_json::Value::String(header.to_string());
            }
            columns.push(col);
        }
        srcs[selected as usize]["columns"] = serde_json::Value::Array(columns);
    }
    Ok(())
}

// ---------- 辅助 ----------

/// 展示用：去掉旧版 `{monthDir}/` 前缀，只显示相对文件名。
fn display_file(raw: &str) -> String {
    normalize_file(raw)
}

/// 归一化：去掉首部 `{monthDir}/` 前缀与首尾空白。
fn normalize_file(raw: &str) -> String {
    let t = raw.trim();
    t.strip_prefix("{monthDir}/")
        .unwrap_or(t)
        .trim()
        .to_string()
}

fn sources_of(doc: &EditorDoc) -> Option<&Vec<serde_json::Value>> {
    doc.raw
        .as_ref()
        .and_then(|r| r.get("scheme"))
        .and_then(|s| s.get("sources"))
        .and_then(|v| v.as_array())
}

fn log_plain(nodes: &EditorNodes, godot: &mut GodotAccess, line: &str) {
    info!("{line}");
    log_line(
        &mut godot.get::<RichTextLabel>(nodes.log),
        LogKind::Plain,
        line,
    );
}

fn log_err(nodes: &EditorNodes, godot: &mut GodotAccess, line: &str) {
    info!("{line}");
    log_line(
        &mut godot.get::<RichTextLabel>(nodes.log),
        LogKind::Err,
        line,
    );
}
