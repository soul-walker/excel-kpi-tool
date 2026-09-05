//! 派生视图编辑页签：派生表（行视图）编辑。
//!
//! 数据模型（raw["scheme"]["derived"]）：
//! ```json
//! [ { "id", "from", "merge": {"on":[...]}, "when", "map": {目标:源},
//!     "group_by", "fields": [ {"name","expr"} ] } ]
//! ```
//! UI：上方派生表（每行：id / from 下拉 / 合并续列 / 过滤 when / 详情 / 删除）；
//! 选中某派生后，下方面板编辑该派生的 分组列 / 列映射 / 字段表达式。
//! `map`/`group`/`fields` 只写回当前选中派生，其余字段整表写回。

use bevy::ecs::observer::On;
use bevy::prelude::{Res, ResMut};
use godot::builtin::Vector2;
use godot::classes::control::SizeFlags;
use godot::classes::{
    Button, GridContainer, Label, LineEdit, OptionButton, RichTextLabel, VBoxContainer,
};
use godot::obj::NewAlloc;
use godot::prelude::Gd;
use godot_bevy::prelude::{GodotAccess, GodotNodeHandle, GodotSignals};
use tracing::info;

use super::editor::{EditorDoc, EditorNodes, EditorRefs};
use super::events::{
    AddDerivedClicked, AddDerivedFieldClicked, AddDerivedMapClicked, DerivedDetailClicked,
    DerivedFieldRemoveClicked, DerivedMapRemoveClicked, DerivedRemoveClicked,
};
use super::view::{LogKind, log_line};

/// 派生列表行控件（Gd 非 Send → NonSend）。
#[derive(Default)]
pub struct DerivedListRows {
    pub id_edits: Vec<Gd<LineEdit>>,
    /// from 下拉（选项为候选表 id）。
    pub from_buttons: Vec<Gd<OptionButton>>,
    /// 合并续列输入（逗号/顿号分隔别名）。
    pub merge_edits: Vec<Gd<LineEdit>>,
    /// 过滤 when 表达式。
    pub when_edits: Vec<Gd<LineEdit>>,
    /// “详情”按钮：(序号, 按钮)。
    pub detail_buttons: Vec<(i32, Gd<Button>)>,
    /// 删除按钮：(序号, 按钮)。
    pub del_buttons: Vec<(i32, Gd<Button>)>,
    pub connected_detail: usize,
    pub connected_del: usize,
}

impl DerivedListRows {
    pub fn clear(&mut self) {
        self.id_edits.clear();
        self.from_buttons.clear();
        self.merge_edits.clear();
        self.when_edits.clear();
        self.detail_buttons.clear();
        self.del_buttons.clear();
        self.connected_detail = 0;
        self.connected_del = 0;
    }
}

/// 当前选中派生的详情控件（Gd 非 Send → NonSend）。
#[derive(Default)]
pub struct DerivedDetail {
    pub group_edit: Option<Gd<LineEdit>>,
    pub map_dst: Vec<Gd<LineEdit>>,
    pub map_src: Vec<Gd<LineEdit>>,
    pub map_del: Vec<(i32, Gd<Button>)>,
    pub map_connected: usize,
    pub field_name: Vec<Gd<LineEdit>>,
    pub field_expr: Vec<Gd<LineEdit>>,
    pub field_del: Vec<(i32, Gd<Button>)>,
    pub field_connected: usize,
}

impl DerivedDetail {
    pub fn clear(&mut self) {
        self.group_edit = None;
        self.map_dst.clear();
        self.map_src.clear();
        self.map_del.clear();
        self.map_connected = 0;
        self.field_name.clear();
        self.field_expr.clear();
        self.field_del.clear();
        self.field_connected = 0;
    }
}

fn clear_children(container: &mut VBoxContainer) {
    let n = container.get_child_count();
    for i in (0..n).rev() {
        if let Some(mut child) = container.get_child(i) {
            child.queue_free();
        }
    }
}

/// 全部候选表 id：sources id + 已声明 derived id（保持声明顺序）。
fn table_candidates(doc: &EditorDoc) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(srcs) = doc.raw.as_ref().and_then(|r| {
        r.get("scheme")
            .and_then(|s| s.get("sources"))
            .and_then(|v| v.as_array())
    }) {
        out.extend(
            srcs.iter()
                .filter_map(|s| s.get("id").and_then(|v| v.as_str()).map(String::from)),
        );
    }
    if let Some(ds) = doc.raw.as_ref().and_then(|r| {
        r.get("scheme")
            .and_then(|s| s.get("derived"))
            .and_then(|v| v.as_array())
    }) {
        out.extend(
            ds.iter()
                .filter_map(|d| d.get("id").and_then(|v| v.as_str()).map(String::from)),
        );
    }
    out
}

fn derived_arr(doc: &EditorDoc) -> Vec<serde_json::Value> {
    doc.raw
        .as_ref()
        .and_then(|r| r.get("scheme"))
        .and_then(|s| s.get("derived"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
}

fn derived_mut(doc: &mut EditorDoc) -> Option<&mut Vec<serde_json::Value>> {
    doc.raw
        .as_mut()
        .and_then(|r| r.get_mut("scheme"))
        .and_then(|s| s.get_mut("derived"))
        .and_then(|v| v.as_array_mut())
}

fn nodes_of(refs: &EditorRefs) -> Option<&EditorNodes> {
    refs.nodes.as_ref()
}

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

/// 重建派生表与选中派生详情（载入/增删/切换时调用）。
pub fn render_all(
    nodes: &EditorNodes,
    doc: &EditorDoc,
    list: &mut DerivedListRows,
    detail: &mut DerivedDetail,
    godot: &mut GodotAccess,
) {
    render_list(nodes, doc, list, godot);
    render_detail(nodes, doc, detail, godot);
}

fn render_list(
    nodes: &EditorNodes,
    doc: &EditorDoc,
    list: &mut DerivedListRows,
    godot: &mut GodotAccess,
) {
    list.clear();
    let ds = derived_arr(doc);
    let candidates = table_candidates(doc);

    let mut box_node = godot.get::<VBoxContainer>(nodes.derived_box);
    clear_children(&mut box_node);
    if ds.is_empty() {
        let mut hint = Label::new_alloc();
        hint.set_name("DerivedEmptyHint");
        hint.set_text("暂无派生：点上方「+ 新增派生」（先有数据源，派生才能引用它）");
        hint.set_modulate(godot::builtin::Color::from_rgb(0.6, 0.6, 0.6));
        box_node.add_child(&hint);
        return;
    }

    let gray = godot::builtin::Color::from_rgb(0.62, 0.62, 0.62);
    let mut grid = GridContainer::new_alloc();
    grid.set_name("DerivedGrid");
    grid.set_columns(7);
    grid.add_theme_constant_override("h_separation", 6);
    grid.add_theme_constant_override("v_separation", 2);
    head_label(&mut grid, "#", 30.0, false, gray);
    head_label(&mut grid, "id", 110.0, false, gray);
    head_label(&mut grid, "from 表", 110.0, false, gray);
    head_label(&mut grid, "合并续列 on", 100.0, false, gray);
    head_label(&mut grid, "过滤 when", 0.0, true, gray);
    head_label(&mut grid, "详情", 0.0, false, gray);
    head_label(&mut grid, "删除", 0.0, false, gray);

    for (i, d) in ds.iter().enumerate() {
        let id = d.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let from = d.get("from").and_then(|v| v.as_str()).unwrap_or("");
        let on = d
            .get("merge")
            .and_then(|m| m.get("on"))
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str())
                    .collect::<Vec<_>>()
                    .join("、")
            })
            .unwrap_or_default();
        let when = d.get("when").and_then(|v| v.as_str()).unwrap_or("");

        let mut idx_label = Label::new_alloc();
        idx_label.set_text(&format!("{}", i + 1));
        idx_label.set_custom_minimum_size(Vector2::new(30.0, 0.0));
        grid.add_child(&idx_label);

        let mut id_edit = LineEdit::new_alloc();
        id_edit.set_text(id);
        id_edit.set_placeholder("id");
        id_edit.set_custom_minimum_size(Vector2::new(110.0, 0.0));
        grid.add_child(&id_edit);

        let mut from_btn = OptionButton::new_alloc();
        for c in &candidates {
            from_btn.add_item(c);
        }
        // 若当前 from 不在候选里，额外补一项，避免载入即丢
        if !candidates.iter().any(|c| c == from) && !from.is_empty() {
            from_btn.add_item(from);
        }
        for (j, c) in candidates.iter().enumerate() {
            if c == from {
                from_btn.select(j as i32);
            }
        }
        grid.add_child(&from_btn);

        let mut merge_edit = LineEdit::new_alloc();
        merge_edit.set_text(&on);
        merge_edit.set_placeholder("如 天数（留空=不合并）");
        merge_edit.set_custom_minimum_size(Vector2::new(110.0, 0.0));
        grid.add_child(&merge_edit);

        let mut when_edit = LineEdit::new_alloc();
        when_edit.set_text(when);
        when_edit.set_placeholder("替上课老师 != ''");
        when_edit.set_h_size_flags(SizeFlags::EXPAND_FILL);
        grid.add_child(&when_edit);

        let mut detail_btn = Button::new_alloc();
        detail_btn.set_text("详情");
        grid.add_child(&detail_btn);

        let mut del_btn = Button::new_alloc();
        del_btn.set_text("删除");
        grid.add_child(&del_btn);

        list.id_edits.push(id_edit);
        list.from_buttons.push(from_btn);
        list.merge_edits.push(merge_edit);
        list.when_edits.push(when_edit);
        list.detail_buttons.push((i as i32, detail_btn));
        list.del_buttons.push((i as i32, del_btn));
    }
    box_node.add_child(&grid);
}

fn render_detail(
    nodes: &EditorNodes,
    doc: &EditorDoc,
    detail: &mut DerivedDetail,
    godot: &mut GodotAccess,
) {
    detail.clear();
    let ds = derived_arr(doc);
    let sel = if ds.is_empty() {
        0
    } else {
        (doc.selected_derived as usize).min(ds.len() - 1)
    };

    let mut title = godot.get::<Label>(nodes.derived_detail_title);
    let mut gb_edit = godot.get::<LineEdit>(nodes.derived_group_edit);
    let mut map_box = godot.get::<VBoxContainer>(nodes.derived_map_box);
    clear_children(&mut map_box);
    let mut field_box = godot.get::<VBoxContainer>(nodes.derived_field_box);
    clear_children(&mut field_box);

    if let Some(d) = ds.get(sel) {
        let id = d.get("id").and_then(|v| v.as_str()).unwrap_or("?");
        title.set_text(&format!("派生详情 · {}（第 {} 个）", id, sel + 1));
        // 分组列
        let gb = d.get("group_by").and_then(|v| v.as_str()).unwrap_or("");
        gb_edit.set_text(gb);
        detail.group_edit = Some(gb_edit);

        // 列映射
        let map = d.get("map").and_then(|m| m.as_object()).cloned();
        if let Some(map) = map {
            if map.is_empty() {
                empty_hint(
                    &mut map_box,
                    "无映射：整行原样透传（点「+ 映射」换列视角）",
                    "DerivedMapHint",
                );
            }
            for (dst, src) in map {
                let (dst_e, src_e, del) = map_row(&mut map_box, &dst, src.as_str().unwrap_or(""));
                let idx = detail.map_del.len() as i32;
                detail.map_dst.push(dst_e);
                detail.map_src.push(src_e);
                detail.map_del.push((idx, del));
            }
        } else {
            empty_hint(
                &mut map_box,
                "无映射：整行原样透传（点「+ 映射」换列视角）",
                "DerivedMapHint",
            );
        }

        // 字段
        let fields = d
            .get("fields")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        if fields.is_empty() {
            empty_hint(
                &mut field_box,
                "无字段：本派生作为明细视图供聚合/分组引用",
                "DerivedFieldHint",
            );
        }
        for f in &fields {
            let name = f.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let expr = f.get("expr").and_then(|v| v.as_str()).unwrap_or("");
            let (n_e, e_e, del) = field_row(&mut field_box, name, expr);
            let idx = detail.field_del.len() as i32;
            detail.field_name.push(n_e);
            detail.field_expr.push(e_e);
            detail.field_del.push((idx, del));
        }
    } else {
        title.set_text("派生详情（先新增派生，点列表行尾「详情」切换）");
        gb_edit.set_text("");
        empty_hint(&mut map_box, "无映射", "DerivedMapHint");
        empty_hint(&mut field_box, "无字段", "DerivedFieldHint");
    }
}

fn empty_hint(box_node: &mut VBoxContainer, text: &str, name: &str) {
    let mut hint = Label::new_alloc();
    hint.set_name(name);
    hint.set_text(text);
    hint.set_modulate(godot::builtin::Color::from_rgb(0.6, 0.6, 0.6));
    box_node.add_child(&hint);
}

/// 映射行：目标别名 / 源列。
fn map_row(
    box_node: &mut VBoxContainer,
    dst: &str,
    src: &str,
) -> (Gd<LineEdit>, Gd<LineEdit>, Gd<Button>) {
    let mut row = GridContainer::new_alloc();
    row.set_columns(3);
    row.add_theme_constant_override("h_separation", 6);
    let mut dst_e = LineEdit::new_alloc();
    dst_e.set_text(dst);
    dst_e.set_placeholder("目标列名");
    dst_e.set_custom_minimum_size(Vector2::new(140.0, 0.0));
    row.add_child(&dst_e);
    let mut src_e = LineEdit::new_alloc();
    src_e.set_text(src);
    src_e.set_placeholder("源列名");
    src_e.set_custom_minimum_size(Vector2::new(140.0, 0.0));
    row.add_child(&src_e);
    let mut del = Button::new_alloc();
    del.set_text("删除");
    row.add_child(&del);
    box_node.add_child(&row);
    (dst_e, src_e, del)
}

fn field_row(
    box_node: &mut VBoxContainer,
    name: &str,
    expr: &str,
) -> (Gd<LineEdit>, Gd<LineEdit>, Gd<Button>) {
    let mut row = GridContainer::new_alloc();
    row.set_columns(3);
    row.add_theme_constant_override("h_separation", 6);
    let mut n_e = LineEdit::new_alloc();
    n_e.set_text(name);
    n_e.set_placeholder("字段名");
    n_e.set_custom_minimum_size(Vector2::new(110.0, 0.0));
    row.add_child(&n_e);
    let mut e_e = LineEdit::new_alloc();
    e_e.set_text(expr);
    e_e.set_placeholder("表达式，如 SUM(节数)");
    e_e.set_h_size_flags(SizeFlags::EXPAND_FILL);
    row.add_child(&e_e);
    let mut del = Button::new_alloc();
    del.set_text("删除");
    row.add_child(&del);
    box_node.add_child(&row);
    (n_e, e_e, del)
}

// ---------- 动态信号连接 ----------

/// 每帧补连新渲染按钮：派生行 详情/删除、详情面板 映射/字段删除。
pub fn connect_derived_signals(
    mut list: bevy::prelude::NonSendMut<DerivedListRows>,
    mut detail: bevy::prelude::NonSendMut<DerivedDetail>,
    detail_sig: GodotSignals<DerivedDetailClicked>,
    del_sig: GodotSignals<DerivedRemoveClicked>,
    map_del_sig: GodotSignals<DerivedMapRemoveClicked>,
    field_del_sig: GodotSignals<DerivedFieldRemoveClicked>,
) {
    while list.connected_detail < list.detail_buttons.len() {
        let (index, button) = list.detail_buttons[list.connected_detail].clone();
        let handle = GodotNodeHandle::from_instance_id(button.instance_id());
        detail_sig.connect(handle, "pressed", None, move |_a, _n, _e| {
            Some(DerivedDetailClicked { index })
        });
        list.connected_detail += 1;
    }
    while list.connected_del < list.del_buttons.len() {
        let (index, button) = list.del_buttons[list.connected_del].clone();
        let handle = GodotNodeHandle::from_instance_id(button.instance_id());
        del_sig.connect(handle, "pressed", None, move |_a, _n, _e| {
            Some(DerivedRemoveClicked { index })
        });
        list.connected_del += 1;
    }
    while detail.map_connected < detail.map_del.len() {
        let (index, button) = detail.map_del[detail.map_connected].clone();
        let handle = GodotNodeHandle::from_instance_id(button.instance_id());
        map_del_sig.connect(handle, "pressed", None, move |_a, _n, _e| {
            Some(DerivedMapRemoveClicked { index })
        });
        detail.map_connected += 1;
    }
    while detail.field_connected < detail.field_del.len() {
        let (index, button) = detail.field_del[detail.field_connected].clone();
        let handle = GodotNodeHandle::from_instance_id(button.instance_id());
        field_del_sig.connect(handle, "pressed", None, move |_a, _n, _e| {
            Some(DerivedFieldRemoveClicked { index })
        });
        detail.field_connected += 1;
    }
}

// ---------- 增删（观察者） ----------

pub fn on_add_derived(
    _trigger: On<AddDerivedClicked>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut list: bevy::prelude::NonSendMut<DerivedListRows>,
    mut detail: bevy::prelude::NonSendMut<DerivedDetail>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = nodes_of(&refs) else { return };
    let candidates = table_candidates(&doc);
    let n = {
        let Some(ds) = derived_mut(&mut doc) else {
            log_err(nodes, &mut godot, "尚未载入方案");
            return;
        };
        ds.push(serde_json::json!({
            "id": format!("派生{}", ds.len() + 1),
            "from": candidates.first().cloned().unwrap_or_default(),
            "when": serde_json::Value::Null,
            "map": {},
            "fields": []
        }));
        ds.len()
    };
    doc.selected_derived = n as i32 - 1;
    render_all(nodes, &doc, &mut list, &mut detail, &mut godot);
    log_plain(
        nodes,
        &mut godot,
        &format!("已新增派生（共 {n} 个）：在下方详情面板编辑映射/字段"),
    );
}

pub fn on_remove_derived(
    trigger: On<DerivedRemoveClicked>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut list: bevy::prelude::NonSendMut<DerivedListRows>,
    mut detail: bevy::prelude::NonSendMut<DerivedDetail>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = nodes_of(&refs) else { return };
    let idx = trigger.event().index;
    let removed_id = {
        let Some(ds) = derived_mut(&mut doc) else {
            return;
        };
        if idx < 0 || idx as usize >= ds.len() {
            return;
        }
        let rid = ds[idx as usize]
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_string();
        ds.remove(idx as usize);
        rid
    };
    let last = derived_arr(&doc).len() as i32 - 1;
    if doc.selected_derived > last {
        doc.selected_derived = last.max(0);
    }
    render_all(nodes, &doc, &mut list, &mut detail, &mut godot);
    log_plain(
        nodes,
        &mut godot,
        &format!("已删除派生 [{idx}]：{removed_id}"),
    );
}

pub fn on_derived_detail(
    trigger: On<DerivedDetailClicked>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut list: bevy::prelude::NonSendMut<DerivedListRows>,
    mut detail: bevy::prelude::NonSendMut<DerivedDetail>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = nodes_of(&refs) else { return };
    doc.selected_derived = trigger.event().index.max(0);
    render_all(nodes, &doc, &mut list, &mut detail, &mut godot);
    let ds = derived_arr(&doc);
    let id = ds
        .get(doc.selected_derived as usize)
        .and_then(|d| d.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    log_plain(nodes, &mut godot, &format!("编辑派生：{id}"));
}

pub fn on_add_map(
    _trigger: On<AddDerivedMapClicked>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut detail: bevy::prelude::NonSendMut<DerivedDetail>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = nodes_of(&refs) else { return };
    let sel = doc.selected_derived;
    let key = {
        let Some(ds) = derived_mut(&mut doc) else {
            return;
        };
        let Some(d) = ds.get_mut(sel as usize) else {
            log_err(nodes, &mut godot, "请先选择一个派生");
            return;
        };
        let obj = d.as_object_mut().expect("derived 应为对象");
        let m = obj
            .entry("map".to_string())
            .or_insert_with(|| serde_json::json!({}));
        let Some(map) = m.as_object_mut() else {
            log_err(nodes, &mut godot, "map 应为对象");
            return;
        };
        let mut n = map.len() + 1;
        loop {
            let key = format!("列{n}");
            if !map.contains_key(&key) {
                map.insert(key.clone(), serde_json::Value::String(String::new()));
                break key;
            }
            n += 1;
        }
    };
    render_detail(nodes, &doc, &mut detail, &mut godot);
    log_plain(
        nodes,
        &mut godot,
        &format!("已加映射 {key}（填 目标列名 与 源列名）"),
    );
}

pub fn on_remove_map(
    trigger: On<DerivedMapRemoveClicked>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut detail: bevy::prelude::NonSendMut<DerivedDetail>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = nodes_of(&refs) else { return };
    let idx = trigger.event().index as usize;
    let sel = doc.selected_derived as usize;
    // 通过渲染行对齐的“目标列名”从 raw 的 map 中移除该键。
    let removed_dst: String = {
        let Some(ds) = derived_mut(&mut doc) else {
            return;
        };
        let Some(d) = ds.get_mut(sel) else { return };
        let Some(map) = d.get_mut("map").and_then(|m| m.as_object_mut()) else {
            return;
        };
        let mut removed = String::new();
        if idx < detail.map_dst.len() {
            let dst = detail.map_dst[idx].get_text().to_string();
            let dst = dst.trim().to_string();
            if !dst.is_empty() && map.remove(&dst).is_some() {
                removed = dst;
            }
        }
        removed
    };
    render_detail(nodes, &doc, &mut detail, &mut godot);
    if removed_dst.is_empty() {
        log_err(nodes, &mut godot, "该行目标列为空，已忽略删除");
    } else {
        log_plain(nodes, &mut godot, &format!("已删除映射：{removed_dst}"));
    }
}

pub fn on_add_field(
    _trigger: On<AddDerivedFieldClicked>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut detail: bevy::prelude::NonSendMut<DerivedDetail>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = nodes_of(&refs) else { return };
    let sel = doc.selected_derived as usize;
    {
        let Some(ds) = derived_mut(&mut doc) else {
            return;
        };
        if sel >= ds.len() {
            log_err(nodes, &mut godot, "请先选择一个派生");
            return;
        }
        let d = &mut ds[sel];
        if d.get("fields").and_then(|f| f.as_array()).is_none() {
            d["fields"] = serde_json::Value::Array(Vec::new());
        }
        let fields = d
            .get_mut("fields")
            .and_then(|f| f.as_array_mut())
            .expect("刚创建");
        let n = fields.len();
        fields.push(serde_json::json!({
            "name": format!("字段{}", n + 1),
            "expr": serde_json::Value::Null
        }));
    }
    render_detail(nodes, &doc, &mut detail, &mut godot);
    log_plain(nodes, &mut godot, "已加一个字段（填 字段名 与 表达式）");
}

pub fn on_remove_field(
    trigger: On<DerivedFieldRemoveClicked>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut detail: bevy::prelude::NonSendMut<DerivedDetail>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = nodes_of(&refs) else { return };
    let idx = trigger.event().index;
    let sel = doc.selected_derived as usize;
    let removed = {
        let Some(ds) = derived_mut(&mut doc) else {
            return;
        };
        let Some(d) = ds.get_mut(sel) else { return };
        let Some(fields) = d.get_mut("fields").and_then(|f| f.as_array_mut()) else {
            return;
        };
        if idx < 0 || idx as usize >= fields.len() {
            return;
        }
        let name = fields[idx as usize]
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_string();
        fields.remove(idx as usize);
        name
    };
    render_detail(nodes, &doc, &mut detail, &mut godot);
    log_plain(nodes, &mut godot, &format!("已删除字段：{removed}"));
}

// ---------- 写回（保存 / 校验前调用） ----------

/// 派生表整行写回 id/from/merge/when；选中派生的 group/map/fields 写回。
pub fn write_back(
    raw: &mut serde_json::Value,
    list: &DerivedListRows,
    detail: &DerivedDetail,
    selected: i32,
) -> Result<(), String> {
    let candidates = list_candidates_from_raw(raw);
    let Some(scheme_node) = raw.get_mut("scheme") else {
        return Err("缺少 scheme".into());
    };
    let Some(ds) = scheme_node
        .get_mut("derived")
        .and_then(|s| s.as_array_mut())
    else {
        return Err("缺少 derived 数组".into());
    };
    if list.id_edits.len() != ds.len() {
        return Err("派生行数与内容不一致，请重新载入后保存".into());
    }
    for (i, d) in ds.iter_mut().enumerate() {
        let id = list.id_edits[i].get_text().to_string();
        let id = id.trim().to_string();
        if id.is_empty() {
            return Err(format!("第 {} 个派生：id 不能为空", i + 1));
        }
        let from_text = list.from_buttons[i].get_text().to_string();
        let from = from_text.trim();
        if from.is_empty() || !candidates.iter().any(|c| c == from) {
            return Err(format!(
                "派生[{id}]：from 必须指向已有数据源或派生（当前 {from_text}）"
            ));
        }
        d["id"] = serde_json::Value::String(id.clone());
        d["from"] = serde_json::Value::String(from.to_string());
        // merge.on：顿号/逗号分隔
        let merge_text = list.merge_edits[i].get_text().to_string();
        let on: Vec<String> = merge_text
            .split(['、', ',', '，'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();
        if on.is_empty() {
            d.as_object_mut().map(|o| o.remove("merge"));
        } else {
            d["merge"] = serde_json::json!({ "on": on });
        }
        // when
        let when_text = list.when_edits[i].get_text().to_string();
        let when = when_text.trim();
        if when.is_empty() {
            d.as_object_mut().map(|o| o.remove("when"));
        } else {
            d["when"] = serde_json::Value::String(when.to_string());
        }
        // 选中派生的详情
        if i == selected as usize {
            // group_by
            let gb = detail
                .group_edit
                .as_ref()
                .map(|e| e.get_text().to_string())
                .unwrap_or_default();
            let gb = gb.trim().to_string();
            if gb.is_empty() {
                d.as_object_mut().map(|o| o.remove("group_by"));
            } else {
                d["group_by"] = serde_json::Value::String(gb);
            }
            // map：目标=源
            let mut map = serde_json::Map::new();
            for j in 0..detail.map_dst.len() {
                let dst = detail.map_dst[j].get_text().to_string();
                let src = detail.map_src[j].get_text().to_string();
                let dst = dst.trim().to_string();
                let src = src.trim().to_string();
                if dst.is_empty() && src.is_empty() {
                    continue;
                }
                if dst.is_empty() || src.is_empty() {
                    return Err(format!("派生[{id}]：映射行 {} 的目标/源列不能为空", j + 1));
                }
                if map.contains_key(&dst) {
                    return Err(format!("派生[{id}]：映射目标重复：{dst}"));
                }
                map.insert(dst, serde_json::Value::String(src));
            }
            d["map"] = serde_json::Value::Object(map);
            // fields
            let mut fields = Vec::new();
            for j in 0..detail.field_name.len() {
                let name = detail.field_name[j].get_text().to_string();
                let name = name.trim().to_string();
                if name.is_empty() {
                    continue;
                }
                let expr = detail.field_expr[j].get_text().to_string();
                let expr = expr.trim().to_string();
                let mut f = serde_json::json!({ "name": name });
                if expr.is_empty() {
                    f["expr"] = serde_json::Value::Null;
                } else {
                    f["expr"] = serde_json::Value::String(expr);
                }
                fields.push(f);
            }
            d["fields"] = serde_json::Value::Array(fields);
        }
    }
    Ok(())
}

fn list_candidates_from_raw(raw: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(scheme) = raw.get("scheme") {
        if let Some(srcs) = scheme.get("sources").and_then(|v| v.as_array()) {
            out.extend(
                srcs.iter()
                    .filter_map(|s| s.get("id").and_then(|v| v.as_str()).map(String::from)),
            );
        }
        if let Some(ds) = scheme.get("derived").and_then(|v| v.as_array()) {
            out.extend(
                ds.iter()
                    .filter_map(|d| d.get("id").and_then(|v| v.as_str()).map(String::from)),
            );
        }
    }
    out
}

// ---------- 辅助 ----------

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
