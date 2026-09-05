//! 派生视图编辑页签。
//!
//! 派生表（每行一个派生）只作导航：`序号 | id | 详情 | 删除`。
//! 选中（点「详情」）后，下方详情面板编辑该派生的全部属性：
//! - 基本：派生 id / 来源表 from（下拉）/ 合并续列 on / 过滤 when；
//! - 视图列 columns（表格：列名 + 表达式）。表达式一律带命名空间前缀：
//!   `{from}.列`（输入行列，可作 SUM(...) 聚合）、`{视图id}.列`（本视图前列）、`参数.x`；
//!   某列写 `GROUP_BY({from}.列)` 即声明为组键（全视图至多一列），其余列按组求值。
//!
//! 保存/校验时仅当前选中派生从详情控件写回；未选中的派生原样保留。

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
    AddDerivedClicked, AddDerivedFieldClicked, DerivedDetailClicked, DerivedFieldRemoveClicked,
    DerivedRemoveClicked,
};
use super::view::{LogKind, log_line};

/// 派生列表（导航用）按钮状态（Gd 非 Send → NonSend）。
#[derive(Default)]
pub struct DerivedListRows {
    /// “详情”按钮：(序号, 按钮)。
    pub detail_buttons: Vec<(i32, Gd<Button>)>,
    /// 删除按钮：(序号, 按钮)。
    pub del_buttons: Vec<(i32, Gd<Button>)>,
    pub connected_detail: usize,
    pub connected_del: usize,
}

impl DerivedListRows {
    pub fn clear(&mut self) {
        self.detail_buttons.clear();
        self.del_buttons.clear();
        self.connected_detail = 0;
        self.connected_del = 0;
    }
}

/// 当前选中派生的详情控件（Gd 非 Send → NonSend）。
#[derive(Default)]
pub struct DerivedDetail {
    /// 派生 id 输入。
    pub id_edit: Option<Gd<LineEdit>>,
    /// from 下拉。
    pub from_btn: Option<Gd<OptionButton>>,
    /// 合并续列输入。
    pub on_edit: Option<Gd<LineEdit>>,
    /// 过滤 when 输入。
    pub when_edit: Option<Gd<LineEdit>>,
    pub field_name: Vec<Gd<LineEdit>>,
    pub field_expr: Vec<Gd<LineEdit>>,
    pub field_del: Vec<(i32, Gd<Button>)>,
    pub field_connected: usize,
}

impl DerivedDetail {
    pub fn clear(&mut self) {
        self.id_edit = None;
        self.from_btn = None;
        self.on_edit = None;
        self.when_edit = None;
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
    if let Some(scheme) = doc.raw.as_ref().and_then(|r| r.get("scheme")) {
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

fn empty_hint(box_node: &mut VBoxContainer, text: &str, name: &str) {
    let mut hint = Label::new_alloc();
    hint.set_name(name);
    hint.set_text(text);
    hint.set_modulate(godot::builtin::Color::from_rgb(0.6, 0.6, 0.6));
    box_node.add_child(&hint);
}

/// 重建派生导航表与详情面板（载入/增删/切换时调用）。
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
    grid.set_columns(4);
    grid.add_theme_constant_override("h_separation", 6);
    grid.add_theme_constant_override("v_separation", 2);
    head_label(&mut grid, "#", 30.0, false, gray);
    head_label(&mut grid, "派生 id（点详情编辑）", 0.0, true, gray);
    head_label(&mut grid, "详情", 0.0, false, gray);
    head_label(&mut grid, "删除", 0.0, false, gray);

    for (i, d) in ds.iter().enumerate() {
        let id = d.get("id").and_then(|v| v.as_str()).unwrap_or("?");

        let mut idx_label = Label::new_alloc();
        idx_label.set_text(&format!("{}", i + 1));
        idx_label.set_custom_minimum_size(Vector2::new(30.0, 0.0));
        grid.add_child(&idx_label);

        let mut id_label = Label::new_alloc();
        id_label.set_text(id);
        id_label.set_h_size_flags(SizeFlags::EXPAND_FILL);
        grid.add_child(&id_label);

        let mut detail_btn = Button::new_alloc();
        detail_btn.set_text("详情");
        grid.add_child(&detail_btn);

        let mut del_btn = Button::new_alloc();
        del_btn.set_text("删除");
        grid.add_child(&del_btn);

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
    let mut id_edit = godot.get::<LineEdit>(nodes.derived_id_edit);
    let mut from_btn = godot.get::<OptionButton>(nodes.derived_from_select);
    let mut on_edit = godot.get::<LineEdit>(nodes.derived_on_edit);
    let mut when_edit = godot.get::<LineEdit>(nodes.derived_when_edit);
    let mut field_box = godot.get::<VBoxContainer>(nodes.derived_field_box);
    clear_children(&mut field_box);

    let candidates = table_candidates(doc);
    from_btn.clear();
    for c in &candidates {
        from_btn.add_item(c);
    }

    if let Some(d) = ds.get(sel) {
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

        title.set_text(&format!("派生详情 · {}（第 {} 个）", id, sel + 1));
        id_edit.set_text(id);
        // from 下拉：候选不存在时补一项再选中
        if !candidates.iter().any(|c| c == from) && !from.is_empty() {
            from_btn.add_item(from);
        }
        for (j, c) in candidates.iter().enumerate() {
            if c == from {
                from_btn.select(j as i32);
            }
        }
        if from.is_empty() {
            from_btn.select(0);
        }
        on_edit.set_text(&on);
        when_edit.set_text(when);

        // 视图列表格（含表头）：列名 + 表达式（expr 空 = 同名透传）
        let columns = d
            .get("columns")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let mut grid = GridContainer::new_alloc();
        grid.set_name("DerivedColGrid");
        grid.set_columns(3);
        grid.add_theme_constant_override("h_separation", 6);
        grid.add_theme_constant_override("v_separation", 2);
        let gray = godot::builtin::Color::from_rgb(0.62, 0.62, 0.62);
        head_label(&mut grid, "列名", 130.0, false, gray);
        head_label(
            &mut grid,
            "表达式（留空=透传同名输入列；{from}.列 / {视图id}.前列 / 参数.x；某列写 GROUP_BY({from}.列) 即分组）",
            0.0,
            true,
            gray,
        );
        head_label(&mut grid, "删除", 0.0, false, gray);
        if columns.is_empty() {
            let mut hint = Label::new_alloc();
            hint.set_text("无视图列：派生原样透传输入行（合并/过滤视图）或点「+ 视图列」定义输出");
            hint.set_modulate(godot::builtin::Color::from_rgb(0.6, 0.6, 0.6));
            grid.add_child(&hint);
        }
        for f in &columns {
            let name = f.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let expr = f.get("expr").and_then(|v| v.as_str()).unwrap_or("");
            let mut n_e = LineEdit::new_alloc();
            n_e.set_text(name);
            n_e.set_placeholder("列名");
            n_e.set_custom_minimum_size(Vector2::new(130.0, 0.0));
            grid.add_child(&n_e);
            let mut e_e = LineEdit::new_alloc();
            e_e.set_text(expr);
            e_e.set_placeholder("如 {from}.列 / SUM({from}.列) / 参数.x");
            e_e.set_h_size_flags(SizeFlags::EXPAND_FILL);
            grid.add_child(&e_e);
            let mut del = Button::new_alloc();
            del.set_text("删除");
            grid.add_child(&del);
            let idx = detail.field_del.len() as i32;
            detail.field_name.push(n_e);
            detail.field_expr.push(e_e);
            detail.field_del.push((idx, del));
        }
        field_box.add_child(&grid);

        detail.id_edit = Some(id_edit);
        detail.from_btn = Some(from_btn);
        detail.on_edit = Some(on_edit);
        detail.when_edit = Some(when_edit);
    } else {
        title.set_text("派生详情（先新增派生）");
        id_edit.set_text("");
        when_edit.set_text("");
        on_edit.set_text("");
        empty_hint(&mut field_box, "无视图列", "DerivedFieldHint");
    }
}

// ---------- 动态信号连接 ----------

/// 每帧补连新渲染按钮：派生行 详情/删除、详情面板 映射/字段删除。
pub fn connect_derived_signals(
    mut list: bevy::prelude::NonSendMut<DerivedListRows>,
    mut detail: bevy::prelude::NonSendMut<DerivedDetail>,
    detail_sig: GodotSignals<DerivedDetailClicked>,
    del_sig: GodotSignals<DerivedRemoveClicked>,
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
            "columns": []
        }));
        ds.len()
    };
    doc.selected_derived = n as i32 - 1;
    render_all(nodes, &doc, &mut list, &mut detail, &mut godot);
    log_plain(
        nodes,
        &mut godot,
        &format!("已新增派生（共 {n} 个）：在详情面板编辑 id/来源/视图列"),
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
        if d.get("columns").and_then(|f| f.as_array()).is_none() {
            d["columns"] = serde_json::Value::Array(Vec::new());
        }
        let columns = d
            .get_mut("columns")
            .and_then(|f| f.as_array_mut())
            .expect("刚创建");
        let n = columns.len();
        columns.push(serde_json::json!({
            "name": format!("列{}", n + 1),
            "expr": serde_json::Value::Null
        }));
    }
    render_detail(nodes, &doc, &mut detail, &mut godot);
    log_plain(
        nodes,
        &mut godot,
        "已加一个视图列（填 列名；表达式留空=同名透传）",
    );
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
        let Some(fields) = d.get_mut("columns").and_then(|f| f.as_array_mut()) else {
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
    log_plain(nodes, &mut godot, &format!("已删除视图列：{removed}"));
}

// ---------- 写回（保存 / 校验前调用） ----------

/// 当前选中派生的详情（id/from/on/when/视图列）整体写回 raw 的派生项。
pub fn write_back(
    raw: &mut serde_json::Value,
    detail: &DerivedDetail,
    selected: i32,
) -> Result<(), String> {
    let candidates = table_candidates_in(raw);
    let Some(ds) = raw
        .get_mut("scheme")
        .and_then(|s| s.get_mut("derived"))
        .and_then(|s| s.as_array_mut())
    else {
        return Err("缺少 derived 数组".into());
    };
    let sel = selected as usize;
    if sel >= ds.len() {
        return Ok(()); // 无可写派生
    }
    // 基本字段
    let id = detail
        .id_edit
        .as_ref()
        .map(|e| e.get_text().to_string())
        .unwrap_or_default();
    let id = id.trim().to_string();
    if id.is_empty() {
        return Err("派生 id 不能为空".into());
    }
    let from = detail
        .from_btn
        .as_ref()
        .map(|b| b.get_text().to_string())
        .unwrap_or_default();
    let from = from.trim().to_string();
    if from.is_empty() || !candidates.iter().any(|c| c == &from) {
        return Err(format!("派生[{id}]：来源 from 必须指向已有数据源或派生"));
    }
    // merge.on
    let on: Vec<String> = detail
        .on_edit
        .as_ref()
        .map(|e| e.get_text().to_string())
        .unwrap_or_default()
        .split(['、', ',', '，'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();
    let when = detail
        .when_edit
        .as_ref()
        .map(|e| e.get_text().to_string())
        .unwrap_or_default();
    let when = when.trim().to_string();

    // 视图列（列名 + 表达式；expr 空 = 同名透传）
    let mut columns = Vec::new();
    let mut seen_names: Vec<String> = Vec::new();
    for j in 0..detail.field_name.len() {
        let name = detail.field_name[j].get_text().to_string();
        let name = name.trim().to_string();
        if name.is_empty() {
            continue;
        }
        if seen_names.iter().any(|n| n == &name) {
            return Err(format!("派生[{id}]：视图列名重复：{name}"));
        }
        seen_names.push(name.clone());
        let expr = detail.field_expr[j].get_text().to_string();
        let expr = expr.trim().to_string();
        let mut col = serde_json::json!({ "name": name });
        if expr.is_empty() {
            col["expr"] = serde_json::Value::Null;
        } else {
            col["expr"] = serde_json::Value::String(expr);
        }
        columns.push(col);
    }

    let mut d = serde_json::json!({ "id": id, "from": from });
    if !on.is_empty() {
        d["merge"] = serde_json::json!({ "on": on });
    }
    if !when.is_empty() {
        d["when"] = serde_json::Value::String(when);
    }
    if !columns.is_empty() {
        d["columns"] = serde_json::Value::Array(columns);
    }
    ds[sel] = d;
    Ok(())
}

fn table_candidates_in(raw: &serde_json::Value) -> Vec<String> {
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
