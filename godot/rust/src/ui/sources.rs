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

use bevy::ecs::observer::On;
use bevy::prelude::{Res, ResMut};
use godot::builtin::Vector2;
use godot::classes::control::SizeFlags;
use godot::classes::{
    Button, CheckButton, HBoxContainer, Label, LineEdit, OptionButton, RichTextLabel, VBoxContainer,
};
use godot::obj::NewAlloc;
use godot::prelude::Gd;
use godot_bevy::prelude::{GodotAccess, GodotNodeHandle, GodotSignals};
use tracing::info;

use super::editor::{EditorDoc, EditorNodes, EditorRefs};
use super::events::{
    AddColClicked, AddSourceClicked, ColRemoveClicked, SourceColsClicked, SourceRemoveClicked,
};
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
}

impl ColRows {
    pub fn clear(&mut self) {
        self.header_edits.clear();
        self.alias_edits.clear();
        self.type_buttons.clear();
        self.key_checks.clear();
        self.del_buttons.clear();
        self.connected = 0;
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
        hint.set_text("暂无数据源：点上方「+ 新增数据源」（默认带 姓名/主键 列）");
        hint.set_modulate(godot::builtin::Color::from_rgb(0.6, 0.6, 0.6));
        box_node.add_child(&hint);
        return;
    }

    for (i, s) in srcs.iter().enumerate() {
        let id = s.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let file = s.get("file").and_then(|v| v.as_str()).unwrap_or("");
        let sheet = s.get("sheet").and_then(|v| v.as_str()).unwrap_or("");

        let mut card = VBoxContainer::new_alloc();
        card.set_name(&format!("SourceCard{i}"));
        card.add_theme_constant_override("separation", 2);

        // 行1：序号 / id / sheet / 列映射 / 删除
        let mut line1 = HBoxContainer::new_alloc();
        line1.add_theme_constant_override("separation", 4);
        let mut idx_label = Label::new_alloc();
        idx_label.set_text(&format!("源{}", i + 1));
        idx_label.set_custom_minimum_size(Vector2::new(38.0, 0.0));
        line1.add_child(&idx_label);

        let mut id_edit = LineEdit::new_alloc();
        id_edit.set_text(id);
        id_edit.set_placeholder("id");
        id_edit.set_h_size_flags(SizeFlags::EXPAND_FILL);
        id_edit.set_tooltip_text("数据源标识（派生/聚合按 id 引用；可用中文，如 课时表）");
        line1.add_child(&id_edit);

        let mut sheet_edit = LineEdit::new_alloc();
        sheet_edit.set_text(sheet);
        sheet_edit.set_placeholder("sheet 名");
        sheet_edit.set_custom_minimum_size(Vector2::new(92.0, 0.0));
        line1.add_child(&sheet_edit);

        let mut col_btn = Button::new_alloc();
        col_btn.set_text("列映射");
        line1.add_child(&col_btn);

        let mut del_btn = Button::new_alloc();
        del_btn.set_text("删除");
        line1.add_child(&del_btn);
        card.add_child(&line1);

        // 行2：相对月份目录的文件名
        let mut line2 = HBoxContainer::new_alloc();
        line2.add_theme_constant_override("separation", 4);
        let mut file_label = Label::new_alloc();
        file_label.set_text("文件");
        file_label.set_custom_minimum_size(Vector2::new(38.0, 0.0));
        line2.add_child(&file_label);
        let mut file_edit = LineEdit::new_alloc();
        file_edit.set_text(&display_file(file));
        file_edit.set_placeholder("课时.xlsx（月份目录下文件名）");
        file_edit.set_h_size_flags(SizeFlags::EXPAND_FILL);
        file_edit.set_tooltip_text(
            "相对运行台所选月份目录的文件名；旧版 {monthDir}/ 前缀保存时自动去掉",
        );
        line2.add_child(&file_edit);
        card.add_child(&line2);

        box_node.add_child(&card);
        rows.id_edits.push(id_edit);
        rows.file_edits.push(file_edit);
        rows.sheet_edits.push(sheet_edit);
        rows.col_buttons.push((i as i32, col_btn));
        rows.del_buttons.push((i as i32, del_btn));
    }
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
        hint.set_text("该源还没有列：点「+ 新增列」添加（表头即 Excel 列标题文字）");
        hint.set_modulate(godot::builtin::Color::from_rgb(0.6, 0.6, 0.6));
        box_node.add_child(&hint);
        return;
    }

    for (j, c) in columns.iter().enumerate() {
        let header = c.get("header").and_then(|v| v.as_str()).unwrap_or("");
        let alias = c.get("alias").and_then(|v| v.as_str()).unwrap_or("");
        let ty = c
            .get("value_type")
            .and_then(|v| v.as_str())
            .unwrap_or("text");
        let key = c.get("key").and_then(|v| v.as_bool()).unwrap_or(false);

        let mut row = HBoxContainer::new_alloc();
        row.set_name(&format!("ColRow{j}"));
        row.add_theme_constant_override("separation", 4);

        let mut header_edit = LineEdit::new_alloc();
        header_edit.set_text(header);
        header_edit.set_placeholder("表头名");
        header_edit.set_h_size_flags(SizeFlags::EXPAND_FILL);
        header_edit.set_tooltip_text("Excel 列标题文字（留空则用别名按列序定位）");
        row.add_child(&header_edit);

        let mut alias_edit = LineEdit::new_alloc();
        alias_edit.set_text(alias);
        alias_edit.set_placeholder("别名");
        alias_edit.set_custom_minimum_size(Vector2::new(130.0, 0.0));
        alias_edit.set_tooltip_text("引擎内引用名（表达式 / key 用），可中文");
        row.add_child(&alias_edit);

        let mut type_btn = OptionButton::new_alloc();
        for (_, label) in COL_TYPES {
            type_btn.add_item(label);
        }
        type_btn.select(col_type_index(ty));
        row.add_child(&type_btn);

        let mut key_check = CheckButton::new_alloc();
        key_check.set_text("主键");
        key_check.set_pressed(key);
        row.add_child(&key_check);

        let mut del_btn = Button::new_alloc();
        del_btn.set_text("删除");
        row.add_child(&del_btn);

        box_node.add_child(&row);
        cols.header_edits.push(header_edit);
        cols.alias_edits.push(alias_edit);
        cols.type_buttons.push(type_btn);
        cols.key_checks.push(key_check);
        cols.del_buttons.push((j as i32, del_btn));
    }
}

// ---------- 动态信号连接 ----------

/// 每帧补连新渲染的按钮。
pub fn connect_sources_signals(
    mut rows: bevy::prelude::NonSendMut<SourceRows>,
    mut cols: bevy::prelude::NonSendMut<ColRows>,
    source_del: GodotSignals<SourceRemoveClicked>,
    source_cols: GodotSignals<SourceColsClicked>,
    col_del: GodotSignals<ColRemoveClicked>,
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
}

// ---------- 增删（观察者） ----------

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
    mut godot: GodotAccess,
) {
    let Some(nodes) = nodes_of(&refs) else { return };
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
    mut godot: GodotAccess,
) {
    let Some(nodes) = nodes_of(&refs) else { return };
    let idx = trigger.event().index;
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
    mut godot: GodotAccess,
) {
    let Some(nodes) = nodes_of(&refs) else { return };
    doc.selected_source = trigger.event().index.max(0);
    render_all(nodes, &doc, &mut rows, &mut cols, &mut godot);
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
    mut godot: GodotAccess,
) {
    let Some(nodes) = nodes_of(&refs) else { return };
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
    mut godot: GodotAccess,
) {
    let Some(nodes) = nodes_of(&refs) else { return };
    let idx = trigger.event().index;
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
