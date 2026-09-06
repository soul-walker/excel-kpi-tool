//! 派生视图编辑页签。
//!
//! 顶部下拉（DerivedPick）列出全部派生，选中即切换详情面板；无列表行按钮。
//! 详情面板编辑选中派生的全部属性：
//! - 基本：派生 id / 来源表 from（下拉）/ 合并续列 on / 行过滤 where；
//! - 视图列 columns（表格：列名 + 表达式）。表达式一律带命名空间前缀：
//!   `{from}.列`（输入行列，可作 SUM(...) 聚合）、`{视图id}.列`（本视图前列）、`参数.x`；
//!   某列写 `GROUP_BY({from}.列)` 即声明为组键（全视图至多一列），其余列按组求值。
//!
//! 保存/校验时仅当前选中派生从详情控件写回；未选中的派生原样保留。
//! 任何会重建详情面板的操作（新增/删除/切换派生、加删视图列）都会先把当前
//! 详情控件内容写回它所属的派生项（`flush_current`），因此编辑不会因重建丢失。

use bevy::ecs::observer::On;
use bevy::prelude::{Res, ResMut};
use godot::builtin::Vector2;
use godot::classes::control::SizeFlags;
use godot::classes::{
    Button, CodeEdit, GridContainer, Label, LineEdit, OptionButton, RichTextLabel, VBoxContainer,
};
use godot::obj::NewAlloc;
use godot::prelude::Gd;
use godot_bevy::prelude::{GodotAccess, GodotNodeHandle, GodotSignals};
use tracing::info;

use super::aggregates::{self, AggregateDetail as AggDetail};
use super::editor::{EditorDoc, EditorNodes, EditorRefs};
use super::events::{
    AddDerivedClicked, AddDerivedFieldClicked, DerivedFieldRemoveClicked, DerivedPicked,
    ExprCompletionRequested, ExprFocusEntered, ExprFocusExited, ExprTextChanged,
    NameFieldFocusLost, RemoveCurrentDerived,
};
use super::expr;
use super::refs;
use super::view::{LogKind, log_line};

/// 当前选中派生的详情控件（Gd 非 Send → NonSend）。
#[derive(Default)]
pub struct DerivedDetail {
    /// 派生 id 输入。
    pub id_edit: Option<Gd<LineEdit>>,
    /// from 下拉。
    pub from_btn: Option<Gd<OptionButton>>,
    /// 合并续列输入。
    pub on_edit: Option<Gd<LineEdit>>,
    /// 行过滤 where 输入（CodeEdit，带补全）。
    pub where_edit: Option<Gd<CodeEdit>>,
    pub field_name: Vec<Gd<LineEdit>>,
    /// 视图列表达式输入（CodeEdit，带补全）。
    pub field_expr: Vec<Gd<CodeEdit>>,
    pub field_del: Vec<(i32, Gd<Button>)>,
    pub field_connected: usize,
    /// where 静态节点已连接的实例（页面重建换新节点后才需重连）。
    pub where_connected: Option<GodotNodeHandle>,
    /// 已连接补全信号的表达式 CodeEdit 数（渲染重建后重新补连）。
    pub expr_connected: usize,
    /// 详情面板当前展示的派生序号（None = 暂无派生/尚未渲染）。
    /// 新增/删除/切换派生与加删视图列前，据此把未保存编辑写回对应派生项。
    pub rendered_for: Option<i32>,
    /// 派生 id 静态输入框已连失焦的实例（改名即同步）。
    pub id_focus_connected: Option<GodotNodeHandle>,
    /// 已连接“视图列名”输入框失焦数。
    pub name_focus_connected: usize,
}

impl DerivedDetail {
    pub fn clear(&mut self) {
        self.id_edit = None;
        self.from_btn = None;
        self.on_edit = None;
        self.where_edit = None;
        self.field_name.clear();
        self.field_expr.clear();
        self.field_del.clear();
        self.field_connected = 0;
        self.expr_connected = 0;
        self.rendered_for = None;
        self.id_focus_connected = None;
        self.name_focus_connected = 0;
        // 注意：where_connected 不清——静态 WhereEdit 节点跨重绘复用，避免重复连接。
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
pub(crate) fn table_candidates(doc: &EditorDoc) -> Vec<String> {
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

/// 重建派生切换下拉与详情面板（载入/新增/删除/切换时调用）。
pub fn render_all(
    nodes: &EditorNodes,
    doc: &EditorDoc,
    detail: &mut DerivedDetail,
    godot: &mut GodotAccess,
) {
    render_picker(nodes, doc, godot);
    render_detail(nodes, doc, detail, godot);
}

/// 把全部派生填进顶部下拉并选中当前项（空列表时禁用，显示占位文案）。
fn render_picker(nodes: &EditorNodes, doc: &EditorDoc, godot: &mut GodotAccess) {
    let ds = derived_arr(doc);
    let mut pick = godot.get::<OptionButton>(nodes.derived_pick);
    pick.clear();
    pick.set_tooltip_text("当前编辑的派生；切换后详情面板即载入它");
    if ds.is_empty() {
        pick.set_disabled(true);
        pick.add_item("暂无派生：点「+ 新增派生」");
        pick.select(0);
        return;
    }
    pick.set_disabled(false);
    for (i, d) in ds.iter().enumerate() {
        let id = d.get("id").and_then(|v| v.as_str()).unwrap_or("?");
        pick.add_item(&format!("{}. {}", i + 1, id));
    }
    let sel = (doc.selected_derived as usize).min(ds.len() - 1);
    pick.select(sel as i32);
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
    detail.rendered_for = if ds.is_empty() {
        None
    } else {
        Some(sel as i32)
    };

    let mut title = godot.get::<Label>(nodes.derived_detail_title);
    let mut id_edit = godot.get::<LineEdit>(nodes.derived_id_edit);
    let mut from_btn = godot.get::<OptionButton>(nodes.derived_from_select);
    let mut on_edit = godot.get::<LineEdit>(nodes.derived_on_edit);
    let mut where_edit = godot.get::<CodeEdit>(nodes.derived_where_edit);
    expr::configure_code_edit(
        &mut where_edit,
        "留空=全部行；引用带前缀，如 qingjia.替上课老师 != ''",
        0.0,
        true,
        doc.raw.as_ref(),
    );
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
        let where_ = d.get("where").and_then(|v| v.as_str()).unwrap_or("");

        title.set_text(&format!(
            "派生详情 · {}（第 {} 个 / 共 {} 个）",
            id,
            sel + 1,
            ds.len()
        ));
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
        where_edit.set_text(where_);

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
        head_label(&mut grid, "列名", 110.0, false, gray);
        // 说明与写法在下方 FieldTitle 已有完整描述，这里保持表头简短，避免把网格撑出横向滚动条。
        head_label(&mut grid, "表达式", 0.0, true, gray);
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
            n_e.set_custom_minimum_size(Vector2::new(110.0, 0.0));
            grid.add_child(&n_e);
            let mut e_e = CodeEdit::new_alloc();
            e_e.set_text(expr);
            e_e.set_h_size_flags(SizeFlags::EXPAND_FILL);
            grid.add_child(&e_e);
            expr::configure_code_edit(
                &mut e_e,
                "如 qingjia.天数 / SUM(qingjia.天数) / 参数.补课时费 / GROUP_BY(qingjia.替上课老师)",
                0.0,
                true,
                doc.raw.as_ref(),
            );
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
        detail.where_edit = Some(where_edit);
    } else {
        title.set_text("派生详情（先新增派生）");
        id_edit.set_text("");
        where_edit.set_text("");
        on_edit.set_text("");
        empty_hint(&mut field_box, "无视图列", "DerivedFieldHint");
    }
}

// ---------- 动态信号连接 ----------

/// 每帧补连新渲染按钮与表达式补全信号：视图列 删除、表达式 CodeEdit。
/// 派生切换/删除是顶部静态节点，由场景信号直接接（见 nav.rs）。
pub fn connect_derived_signals(
    mut detail: bevy::prelude::NonSendMut<DerivedDetail>,
    field_del_sig: GodotSignals<DerivedFieldRemoveClicked>,
    text_sig: GodotSignals<ExprTextChanged>,
    comp_sig: GodotSignals<ExprCompletionRequested>,
    enter_sig: GodotSignals<ExprFocusEntered>,
    exit_sig: GodotSignals<ExprFocusExited>,
    name_focus: GodotSignals<NameFieldFocusLost>,
) {
    while detail.field_connected < detail.field_del.len() {
        let (index, button) = detail.field_del[detail.field_connected].clone();
        let handle = GodotNodeHandle::from_instance_id(button.instance_id());
        field_del_sig.connect(handle, "pressed", None, move |_a, _n, _e| {
            Some(DerivedFieldRemoveClicked { index })
        });
        detail.field_connected += 1;
    }
    // where 静态 CodeEdit：同一实例只连一次（页面重建换实例后才重连）。
    if let Some(edit) = detail.where_edit.clone() {
        let handle = GodotNodeHandle::new(edit);
        if detail.where_connected != Some(handle) {
            expr::connect_code_edit_signals(&text_sig, &comp_sig, &enter_sig, &exit_sig, handle);
            detail.where_connected = Some(handle);
        }
    }
    // 视图列表达式 CodeEdit：渲染重建后把新节点补连。
    while detail.expr_connected < detail.field_expr.len() {
        let edit = detail.field_expr[detail.expr_connected].clone();
        let handle = GodotNodeHandle::new(edit);
        expr::connect_code_edit_signals(&text_sig, &comp_sig, &enter_sig, &exit_sig, handle);
        detail.expr_connected += 1;
    }
    // 派生 id / 视图列名输入框失焦 → 改名即同步。
    if let Some(edit) = detail.id_edit.clone() {
        let handle = GodotNodeHandle::new(edit);
        if detail.id_focus_connected != Some(handle) {
            name_focus.connect(handle, "focus_exited", None, move |_a, n, _e| {
                Some(NameFieldFocusLost { node: n })
            });
            detail.id_focus_connected = Some(handle);
        }
    }
    while detail.name_focus_connected < detail.field_name.len() {
        let edit = detail.field_name[detail.name_focus_connected].clone();
        let handle = GodotNodeHandle::new(edit);
        name_focus.connect(handle, "focus_exited", None, move |_a, n, _e| {
            Some(NameFieldFocusLost { node: n })
        });
        detail.name_focus_connected += 1;
    }
}

// ---------- 增删（观察者） ----------

/// 增删/切换前的“落库”：把详情面板当前内容合并写回 raw 的“正在展示”派生项。
/// 与 write_back 相同的字段归一（空 where/merge/columns 不出现在 JSON），
/// 但不做 id/from 合法性校验——保存/校验时仍会校验，这里只为不丢编辑。
fn flush_current(raw: &mut serde_json::Value, detail: &DerivedDetail) {
    let Some(idx) = detail.rendered_for else {
        return;
    };
    let Some(ds) = raw
        .get_mut("scheme")
        .and_then(|s| s.get_mut("derived"))
        .and_then(|s| s.as_array_mut())
    else {
        return;
    };
    let Some(d) = ds.get_mut(idx as usize) else {
        return;
    };
    let Some(obj) = d.as_object_mut() else {
        return;
    };

    let id = detail
        .id_edit
        .as_ref()
        .map(|e| e.get_text().to_string())
        .unwrap_or_default();
    let id = id.trim().to_string();
    let from = detail
        .from_btn
        .as_ref()
        .map(|b| b.get_text().to_string())
        .unwrap_or_default();
    let from = from.trim().to_string();
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
    let where_ = detail
        .where_edit
        .as_ref()
        .map(|e| e.get_text().to_string())
        .unwrap_or_default();
    let where_ = where_.trim().to_string();

    // 视图列：与 write_back 一致，空列名行跳过；expr 留空 = 同名透传。
    let mut columns = Vec::new();
    for j in 0..detail.field_name.len() {
        let name = detail.field_name[j].get_text().to_string();
        let name = name.trim().to_string();
        if name.is_empty() {
            continue;
        }
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

    obj.insert("id".into(), serde_json::Value::String(id));
    obj.insert("from".into(), serde_json::Value::String(from));
    if on.is_empty() {
        obj.remove("merge");
    } else {
        obj.insert("merge".into(), serde_json::json!({ "on": on }));
    }
    if where_.is_empty() {
        obj.remove("where");
    } else {
        obj.insert("where".into(), serde_json::Value::String(where_));
    }
    if columns.is_empty() {
        obj.remove("columns");
    } else {
        obj.insert("columns".into(), serde_json::Value::Array(columns));
    }
}

/// 增删/切换派生或视图列前调用：先落库再改动模型并重建页面。
pub(crate) fn flush_open(doc: &mut EditorDoc, detail: &DerivedDetail) {
    let Some(raw) = doc.raw.as_mut() else {
        return;
    };
    flush_current(raw, detail);
}

/// 若刚写入的派生详情含改名（派生 id / 视图列名），同步全方案引用并返回日志。
/// 应在 flush_open 之后、模型增删之前调用（行数未变，检测才可靠）。
fn flush_propagate(before: &Option<serde_json::Value>, doc: &mut EditorDoc) -> Vec<String> {
    match (before.as_ref(), doc.raw.as_mut()) {
        (Some(b), Some(a)) => refs::propagate(b, a),
        _ => Vec::new(),
    }
}

pub fn on_add_derived(
    _trigger: On<AddDerivedClicked>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut detail: bevy::prelude::NonSendMut<DerivedDetail>,
    mut adetail: bevy::prelude::NonSendMut<AggDetail>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = nodes_of(&refs) else { return };
    // 先把当前详情面板未保存的编辑提交回 raw，避免新增/重建把上一个派生的编辑冲掉；
    // 聚合页面板也一并落库（它的编辑同样要参与改名传播）。
    let before = doc.raw.clone();
    flush_open(&mut doc, &detail);
    aggregates::flush_open(&mut doc, &adetail);
    let renames = flush_propagate(&before, &mut doc);
    let candidates = table_candidates(&doc);
    let n = {
        let Some(ds) = derived_mut(&mut doc) else {
            log_err(nodes, &mut godot, "尚未载入方案");
            return;
        };
        ds.push(serde_json::json!({
            "id": format!("派生{}", ds.len() + 1),
            "from": candidates.first().cloned().unwrap_or_default(),
            "where": serde_json::Value::Null,
            "columns": []
        }));
        ds.len()
    };
    doc.selected_derived = n as i32 - 1;
    render_all(nodes, &doc, &mut detail, &mut godot);
    if !renames.is_empty() {
        aggregates::render_all(nodes, &doc, &mut adetail, &mut godot);
    }
    for line in &renames {
        log_plain(nodes, &mut godot, line);
    }
    log_plain(
        nodes,
        &mut godot,
        &format!("已新增派生（共 {n} 个）：在详情面板编辑 id/来源/视图列"),
    );
}

/// 顶部下拉切换了要编辑的派生（序号 = 控件当前选中项）。
pub fn on_derived_picked(
    _trigger: On<DerivedPicked>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut detail: bevy::prelude::NonSendMut<DerivedDetail>,
    mut adetail: bevy::prelude::NonSendMut<AggDetail>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = nodes_of(&refs) else { return };
    let count = derived_arr(&doc).len();
    if count == 0 {
        return;
    }
    let idx = godot
        .get::<OptionButton>(nodes.derived_pick)
        .get_selected()
        .max(0) as usize;
    if idx >= count {
        return;
    }
    // 切换选中前先把上一个派生详情里的未保存编辑落回 raw，切回时仍在。
    let before = doc.raw.clone();
    flush_open(&mut doc, &detail);
    aggregates::flush_open(&mut doc, &adetail);
    let renames = flush_propagate(&before, &mut doc);
    doc.selected_derived = idx as i32;
    render_all(nodes, &doc, &mut detail, &mut godot);
    if !renames.is_empty() {
        aggregates::render_all(nodes, &doc, &mut adetail, &mut godot);
    }
    let ds = derived_arr(&doc);
    let id = ds
        .get(doc.selected_derived as usize)
        .and_then(|d| d.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    for line in &renames {
        log_plain(nodes, &mut godot, line);
    }
    log_plain(nodes, &mut godot, &format!("编辑派生：{id}"));
}

/// 「删除当前派生」：删除顶部下拉当前选中的派生。
pub fn on_remove_current(
    _trigger: On<RemoveCurrentDerived>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut detail: bevy::prelude::NonSendMut<DerivedDetail>,
    mut adetail: bevy::prelude::NonSendMut<AggDetail>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = nodes_of(&refs) else { return };
    // 先把当前详情面板未保存的编辑提交回 raw，避免删除重建时丢失。
    let before = doc.raw.clone();
    flush_open(&mut doc, &detail);
    aggregates::flush_open(&mut doc, &adetail);
    let renames = flush_propagate(&before, &mut doc);
    let idx = doc.selected_derived.max(0) as usize;
    let removed_id = {
        let Some(ds) = derived_mut(&mut doc) else {
            return;
        };
        if idx >= ds.len() {
            return;
        }
        let rid = ds[idx]
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_string();
        ds.remove(idx);
        rid
    };
    let last = derived_arr(&doc).len() as i32 - 1;
    if doc.selected_derived > last {
        doc.selected_derived = last.max(0);
    }
    render_all(nodes, &doc, &mut detail, &mut godot);
    if !renames.is_empty() {
        aggregates::render_all(nodes, &doc, &mut adetail, &mut godot);
    }
    for line in &renames {
        log_plain(nodes, &mut godot, line);
    }
    log_plain(nodes, &mut godot, &format!("已删除派生：{removed_id}"));
}

pub fn on_add_field(
    _trigger: On<AddDerivedFieldClicked>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut detail: bevy::prelude::NonSendMut<DerivedDetail>,
    mut adetail: bevy::prelude::NonSendMut<AggDetail>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = nodes_of(&refs) else { return };
    // 先把详情面板未保存的编辑提交回 raw，避免重建列网格把已编辑列还原。
    let before = doc.raw.clone();
    flush_open(&mut doc, &detail);
    aggregates::flush_open(&mut doc, &adetail);
    let renames = flush_propagate(&before, &mut doc);
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
    render_all(nodes, &doc, &mut detail, &mut godot);
    if !renames.is_empty() {
        aggregates::render_all(nodes, &doc, &mut adetail, &mut godot);
    }
    for line in &renames {
        log_plain(nodes, &mut godot, line);
    }
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
    mut adetail: bevy::prelude::NonSendMut<AggDetail>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = nodes_of(&refs) else { return };
    let idx = trigger.event().index;
    // 先把详情面板未保存的编辑提交回 raw，避免删列重建把其它已编辑列还原。
    let before = doc.raw.clone();
    flush_open(&mut doc, &detail);
    aggregates::flush_open(&mut doc, &adetail);
    let renames = flush_propagate(&before, &mut doc);
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
    render_all(nodes, &doc, &mut detail, &mut godot);
    if !renames.is_empty() {
        aggregates::render_all(nodes, &doc, &mut adetail, &mut godot);
    }
    for line in &renames {
        log_plain(nodes, &mut godot, line);
    }
    log_plain(nodes, &mut godot, &format!("已删除视图列：{removed}"));
}

// ---------- 写回（保存 / 校验前调用） ----------

/// 当前选中派生的详情（id/from/on/where/视图列）整体写回 raw 的派生项。
/// from 是否指向有效表不在这里校验：改名传播（refs::propagate）会先按“新旧 id
/// 对照”把 from/表达式里的旧引用改掉，随后由 refs::check_table_refs 统一查。
pub fn write_back(
    raw: &mut serde_json::Value,
    detail: &DerivedDetail,
    selected: i32,
) -> Result<(), String> {
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
    let where_ = detail
        .where_edit
        .as_ref()
        .map(|e| e.get_text().to_string())
        .unwrap_or_default();
    let where_ = where_.trim().to_string();

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
    if !where_.is_empty() {
        d["where"] = serde_json::Value::String(where_);
    }
    if !columns.is_empty() {
        d["columns"] = serde_json::Value::Array(columns);
    }
    ds[sel] = d;
    Ok(())
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
