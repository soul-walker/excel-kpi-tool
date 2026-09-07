//! 计算分组编辑页签。
//!
//! 顶部下拉（GroupPick）列出全部分组，选中即切换详情面板；无列表行按钮。
//! 分组模型即 raw["scheme"]["groups"] 数组项：
//! - 分组 id（输出 groups[].group 按此指向本组；改名失焦即经 refs 全方案同步）；
//! - 名称 name / 匹配 sheet match_sheet（可空，空值写回时省略该键）；
//! - 字段表：第 1 组（`fields`）是“共享基础字段”，其它组（`overrides`）表达
//!   对第 1 组字段的覆盖（表达式留空 = 停用该字段，表达式非空 = 覆盖公式）。
//!
//! 语义与引擎 `effective_fields` 保持一致：永远以 groups[0].fields 为基础，
//! 各组用自己的 overrides 覆盖/停用/追加。因此编辑器只在选中第 1 组时编辑
//! fields 行，其它组编辑 overrides 行；其余数组项原样保留。
//!
//! 详情面板的重建时机同派生页：任何会重建面板的操作（新增/删除/切换分组、
//! 加删字段行）都先把当前面板内容写回它所属的分组项（`flush_open`），
//! 因此编辑不会因重建丢失。

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

use super::aggregates;
use super::derived;
use super::editor::{EditorDoc, EditorNodes, EditorRefs};
use super::events::{
    AddGroupClicked, AddGroupItemClicked, ExprCompletionRequested, ExprFocusEntered,
    ExprFocusExited, ExprTextChanged, GroupItemRemoveClicked, GroupPicked, NameFieldFocusLost,
    RemoveCurrentGroup,
};
use super::expr;
use super::refs;
use super::view::{LogKind, log_line};

/// 当前选中分组的详情控件（Gd 非 Send → NonSend）。
#[derive(Default)]
pub struct GroupDetail {
    /// 分组 id 输入。
    pub id_edit: Option<Gd<LineEdit>>,
    /// 名称输入。
    pub name_edit: Option<Gd<LineEdit>>,
    /// 匹配 sheet 输入。
    pub sheet_edit: Option<Gd<LineEdit>>,
    /// 字段表行：字段名（基础字段/被覆盖字段名）。
    pub item_name: Vec<Gd<LineEdit>>,
    /// 字段表行：表达式输入（CodeEdit，带补全；覆盖行留空 = 停用）。
    pub item_expr: Vec<Gd<CodeEdit>>,
    pub item_del: Vec<(i32, Gd<Button>)>,
    pub item_del_connected: usize,
    pub item_expr_connected: usize,
    /// 已连接“字段名”输入框失焦数（改名即同步）。
    pub item_name_focus_connected: usize,
    /// 分组 id 静态输入框已连失焦的实例。
    pub id_focus_connected: Option<GodotNodeHandle>,
    /// 详情面板当前展示的分组序号（None = 暂无分组/尚未渲染）。
    /// 增删/切换与加删字段行前，据此把未保存编辑写回对应分组项。
    pub rendered_for: Option<i32>,
}

impl GroupDetail {
    pub fn clear(&mut self) {
        self.id_edit = None;
        self.name_edit = None;
        self.sheet_edit = None;
        self.item_name.clear();
        self.item_expr.clear();
        self.item_del.clear();
        self.item_del_connected = 0;
        self.item_expr_connected = 0;
        self.item_name_focus_connected = 0;
        self.id_focus_connected = None;
        self.rendered_for = None;
    }
}

/// 分组是否“基组”（第 1 组）：只有它的 fields 参与引擎计算。
fn is_base(idx: i32) -> bool {
    idx == 0
}

fn group_arr(doc: &EditorDoc) -> Vec<serde_json::Value> {
    doc.raw
        .as_ref()
        .and_then(|r| r.get("scheme"))
        .and_then(|s| s.get("groups"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
}

fn group_mut(doc: &mut EditorDoc) -> Option<&mut Vec<serde_json::Value>> {
    doc.raw
        .as_mut()
        .and_then(|r| r.get_mut("scheme"))
        .and_then(|s| s.get_mut("groups"))
        .and_then(|v| v.as_array_mut())
}

fn nodes_of(refs: &EditorRefs) -> Option<&EditorNodes> {
    refs.nodes.as_ref()
}

/// 取某个分组对象里的基础字段/覆盖数组（仅作展示语义判定用）。
fn base_fields_of(g: &serde_json::Value) -> Vec<serde_json::Value> {
    g.get("fields")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
}

fn overrides_of(g: &serde_json::Value) -> Vec<serde_json::Value> {
    g.get("overrides")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
}

fn empty_hint(box_node: &mut VBoxContainer, text: &str) {
    let mut hint = Label::new_alloc();
    hint.set_name("GroupItemsHint");
    hint.set_text(text);
    hint.set_modulate(godot::builtin::Color::from_rgb(0.6, 0.6, 0.6));
    box_node.add_child(&hint);
}

/// 重建分组切换下拉与详情面板（载入/新增/删除/切换时调用）。
pub fn render_all(
    nodes: &EditorNodes,
    doc: &EditorDoc,
    detail: &mut GroupDetail,
    godot: &mut GodotAccess,
) {
    render_picker(nodes, doc, godot);
    render_detail(nodes, doc, detail, godot);
}

/// 把全部分组填进顶部下拉并选中当前项（空列表时禁用，显示占位文案）。
fn render_picker(nodes: &EditorNodes, doc: &EditorDoc, godot: &mut GodotAccess) {
    let gs = group_arr(doc);
    let mut pick = godot.get::<OptionButton>(nodes.group_pick);
    pick.clear();
    pick.set_tooltip_text("当前编辑的分组；切换后详情面板即载入它");
    if gs.is_empty() {
        pick.set_disabled(true);
        pick.add_item("暂无分组：点「+ 新增分组」");
        pick.select(0);
        return;
    }
    pick.set_disabled(false);
    for (i, g) in gs.iter().enumerate() {
        let id = g.get("id").and_then(|v| v.as_str()).unwrap_or("?");
        let name = g.get("name").and_then(|v| v.as_str()).unwrap_or("").trim();
        let label = if name.is_empty() || name == id {
            format!("{}. {}", i + 1, id)
        } else {
            format!("{}. {}（{name}）", i + 1, id)
        };
        pick.add_item(&label);
    }
    let sel = (doc.selected_group as usize).min(gs.len() - 1);
    pick.select(sel as i32);
}

/// 重填详情面板内容：分组 id/名称/匹配 sheet + 动态字段表（第 1 组=基础字段，其它组=覆盖）。
fn render_detail(
    nodes: &EditorNodes,
    doc: &EditorDoc,
    detail: &mut GroupDetail,
    godot: &mut GodotAccess,
) {
    detail.clear();
    let gs = group_arr(doc);
    let sel = if gs.is_empty() {
        0
    } else {
        (doc.selected_group as usize).min(gs.len() - 1)
    };
    detail.rendered_for = if gs.is_empty() {
        None
    } else {
        Some(sel as i32)
    };

    let mut title = godot.get::<Label>(nodes.group_detail_title);
    let mut id_edit = godot.get::<LineEdit>(nodes.group_id_edit);
    let mut name_edit = godot.get::<LineEdit>(nodes.group_name_edit);
    let mut sheet_edit = godot.get::<LineEdit>(nodes.group_sheet_edit);
    let mut hint = godot.get::<Label>(nodes.group_item_hint);
    let mut add_btn = godot.get::<Button>(nodes.group_add_item_btn);
    let mut item_box = godot.get::<VBoxContainer>(nodes.group_item_box);
    let child_count = item_box.get_child_count();
    for i in (0..child_count).rev() {
        if let Some(mut child) = item_box.get_child(i) {
            child.queue_free();
        }
    }

    let Some(g) = gs.get(sel) else {
        title.set_text("分组详情（先新增分组）");
        id_edit.set_text("");
        name_edit.set_text("");
        sheet_edit.set_text("");
        hint.set_text("暂无分组：点「+ 新增分组」后编辑字段公式");
        add_btn.set_text("+ 新增一行");
        empty_hint(&mut item_box, "无字段");
        return;
    };

    let id = g.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let name = g.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let sheet = g.get("match_sheet").and_then(|v| v.as_str()).unwrap_or("");
    title.set_text(&format!(
        "分组详情 · {id}（第 {} 个 / 共 {} 个）{}",
        sel + 1,
        gs.len(),
        if sel == 0 {
            " · 基础组（共享字段）"
        } else {
            ""
        }
    ));
    id_edit.set_text(id);
    name_edit.set_text(name);
    sheet_edit.set_text(sheet);

    let base = is_base(sel as i32);
    hint.set_text(if base {
        "基础字段（第 1 组 = 全部分组的共享公式；后续组的覆盖以此为准。字段名必填、表达式必填，可引用本组前列 / 参数.x / 聚合.x / {表}.列）"
    } else {
        "覆盖（相对第 1 组字段的增量）：字段名填第 1 组里的字段名；表达式留空 = 停用该字段，非空 = 覆盖公式。也可引用第 1 组字段名 / 参数.x / 聚合.x / {表}.列"
    });
    add_btn.set_text(if base {
        "+ 新增基础字段"
    } else {
        "+ 新增覆盖"
    });

    // 字段表（字段名 + 表达式 + 删除）。
    let rows = if base {
        base_fields_of(g)
    } else {
        overrides_of(g)
    };
    let mut grid = GridContainer::new_alloc();
    grid.set_name("GroupItemGrid");
    grid.set_columns(3);
    grid.add_theme_constant_override("h_separation", 6);
    grid.add_theme_constant_override("v_separation", 2);
    let gray = godot::builtin::Color::from_rgb(0.62, 0.62, 0.62);
    let mut head = Label::new_alloc();
    head.set_text(if base {
        "字段名"
    } else {
        "覆盖的字段名"
    });
    head.set_modulate(gray);
    head.set_custom_minimum_size(Vector2::new(140.0, 0.0));
    grid.add_child(&head);
    let mut head = Label::new_alloc();
    head.set_text("表达式（覆盖行留空 = 停用）");
    head.set_modulate(gray);
    head.set_h_size_flags(SizeFlags::EXPAND_FILL);
    grid.add_child(&head);
    let mut head = Label::new_alloc();
    head.set_text("删除");
    head.set_modulate(gray);
    grid.add_child(&head);

    if rows.is_empty() {
        let mut hint = Label::new_alloc();
        hint.set_name("GroupItemsHint");
        hint.set_text(if base {
            "无基础字段：点「+ 新增基础字段」开始定义（第 1 组必填公式）"
        } else {
            "无覆盖：本组完全继承第 1 组字段"
        });
        hint.set_modulate(godot::builtin::Color::from_rgb(0.6, 0.6, 0.6));
        grid.add_child(&hint);
    }
    for f in &rows {
        let fname = f.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let fexpr = f.get("expr").and_then(|v| v.as_str()).unwrap_or("");
        let mut n_e = LineEdit::new_alloc();
        n_e.set_text(fname);
        n_e.set_placeholder(if base {
            "字段名"
        } else {
            "第 1 组的字段名"
        });
        n_e.set_custom_minimum_size(Vector2::new(140.0, 0.0));
        grid.add_child(&n_e);
        let mut e_e = CodeEdit::new_alloc();
        e_e.set_text(fexpr);
        e_e.set_h_size_flags(SizeFlags::EXPAND_FILL);
        grid.add_child(&e_e);
        expr::configure_code_edit(
            &mut e_e,
            if base {
                "如 课时S * 参数.课时费 / if 聚合.请假超额 { 0 } else { 空 }"
            } else {
                "留空 = 停用；如 ROUND(合计, 0) / if 参数.是否折扣 { … } else { … }"
            },
            0.0,
            true,
            doc.raw.as_ref(),
        );
        let mut del = Button::new_alloc();
        del.set_text("删除");
        grid.add_child(&del);
        let idx = detail.item_del.len() as i32;
        detail.item_name.push(n_e);
        detail.item_expr.push(e_e);
        detail.item_del.push((idx, del));
    }
    item_box.add_child(&grid);

    detail.id_edit = Some(id_edit);
    detail.name_edit = Some(name_edit);
    detail.sheet_edit = Some(sheet_edit);
}

// ---------- 动态信号连接 ----------

/// 每帧补连新渲染行：删除按钮、表达式 CodeEdit 补全、字段名输入框失焦（改名即同步）。
pub fn connect_groups_signals(
    mut detail: bevy::prelude::NonSendMut<GroupDetail>,
    del_sig: GodotSignals<GroupItemRemoveClicked>,
    text_sig: GodotSignals<ExprTextChanged>,
    comp_sig: GodotSignals<ExprCompletionRequested>,
    enter_sig: GodotSignals<ExprFocusEntered>,
    exit_sig: GodotSignals<ExprFocusExited>,
    name_focus: GodotSignals<NameFieldFocusLost>,
) {
    while detail.item_del_connected < detail.item_del.len() {
        let (index, button) = detail.item_del[detail.item_del_connected].clone();
        let handle = GodotNodeHandle::from_instance_id(button.instance_id());
        del_sig.connect(handle, "pressed", None, move |_a, _n, _e| {
            Some(GroupItemRemoveClicked { index })
        });
        detail.item_del_connected += 1;
    }
    while detail.item_expr_connected < detail.item_expr.len() {
        let edit = detail.item_expr[detail.item_expr_connected].clone();
        let handle = GodotNodeHandle::new(edit);
        expr::connect_code_edit_signals(&text_sig, &comp_sig, &enter_sig, &exit_sig, handle);
        detail.item_expr_connected += 1;
    }
    if let Some(edit) = detail.id_edit.clone() {
        let handle = GodotNodeHandle::new(edit);
        if detail.id_focus_connected != Some(handle) {
            name_focus.connect(handle, "focus_exited", None, move |_a, n, _e| {
                Some(NameFieldFocusLost { node: n })
            });
            detail.id_focus_connected = Some(handle);
        }
    }
    while detail.item_name_focus_connected < detail.item_name.len() {
        let edit = detail.item_name[detail.item_name_focus_connected].clone();
        let handle = GodotNodeHandle::new(edit);
        name_focus.connect(handle, "focus_exited", None, move |_a, n, _e| {
            Some(NameFieldFocusLost { node: n })
        });
        detail.item_name_focus_connected += 1;
    }
}

// ---------- 增删/切换（观察者） ----------

/// 分组字段行（字段名 + 表达式文本；覆盖行空表达式表示停用）。
fn collect_items(detail: &GroupDetail) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for j in 0..detail.item_name.len() {
        let name = detail.item_name[j].get_text().to_string();
        let name = name.trim().to_string();
        if name.is_empty() {
            continue;
        }
        let expr = detail.item_expr[j].get_text().to_string();
        out.push((name, expr.trim().to_string()));
    }
    out
}

/// 把当前行写到 {fields|overrides} 数组（整表替换）；空数组省略键。
fn write_items(
    obj: &mut serde_json::Map<String, serde_json::Value>,
    base: bool,
    items: Vec<(String, String)>,
) {
    let key = if base { "fields" } else { "overrides" };
    let arr: Vec<serde_json::Value> = items
        .into_iter()
        .map(|(name, expr)| {
            let mut f = serde_json::json!({ "name": name });
            if expr.is_empty() {
                f["expr"] = serde_json::Value::Null;
            } else {
                f["expr"] = serde_json::Value::String(expr);
            }
            f
        })
        .collect();
    if arr.is_empty() {
        obj.remove(key);
    } else {
        obj.insert(key.into(), serde_json::Value::Array(arr));
    }
}

/// 增删/切换分组或字段行前的“落库”：把详情面板当前内容合并写回 raw 的“正在展示”分组项。
/// 与 write_back 相同的字段归一（空 match_sheet/字段表不出现在 JSON），但不做校验——
/// 保存/校验时仍会校验，这里只为不丢编辑。
fn flush_current(raw: &mut serde_json::Value, detail: &GroupDetail) {
    let Some(idx) = detail.rendered_for else {
        return;
    };
    let Some(gs) = raw
        .get_mut("scheme")
        .and_then(|s| s.get_mut("groups"))
        .and_then(|s| s.as_array_mut())
    else {
        return;
    };
    let Some(g) = gs.get_mut(idx as usize) else {
        return;
    };
    let Some(obj) = g.as_object_mut() else {
        return;
    };

    let id = detail
        .id_edit
        .as_ref()
        .map(|e| e.get_text().to_string())
        .unwrap_or_default();
    let id = id.trim().to_string();
    let name = detail
        .name_edit
        .as_ref()
        .map(|e| e.get_text().to_string())
        .unwrap_or_default();
    let name = name.trim().to_string();
    let sheet = detail
        .sheet_edit
        .as_ref()
        .map(|e| e.get_text().to_string())
        .unwrap_or_default();
    let sheet = sheet.trim().to_string();

    obj.insert("id".into(), serde_json::Value::String(id));
    obj.insert("name".into(), serde_json::Value::String(name));
    if sheet.is_empty() {
        obj.remove("match_sheet");
    } else {
        obj.insert("match_sheet".into(), serde_json::Value::String(sheet));
    }
    write_items(obj, is_base(idx), collect_items(detail));
}

/// 增删/切换分组或字段行前调用：先落库再改动模型并重建页面。
pub(crate) fn flush_open(doc: &mut EditorDoc, detail: &GroupDetail) {
    let Some(raw) = doc.raw.as_mut() else {
        return;
    };
    flush_current(raw, detail);
}

/// 若刚写入的分组详情含改名（分组 id / 基础字段名），同步全方案引用并返回日志。
/// 应在 flush_open 之后、模型增删之前调用（数量未变，检测才可靠）。
fn flush_propagate(before: &Option<serde_json::Value>, doc: &mut EditorDoc) -> Vec<String> {
    match (before.as_ref(), doc.raw.as_mut()) {
        (Some(b), Some(a)) => refs::propagate(b, a),
        _ => Vec::new(),
    }
}

pub fn on_add_group(
    _trigger: On<AddGroupClicked>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut detail: bevy::prelude::NonSendMut<GroupDetail>,
    ddetail: bevy::prelude::NonSend<derived::DerivedDetail>,
    adetail: bevy::prelude::NonSend<aggregates::AggregateDetail>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = nodes_of(&refs) else { return };
    // 先把当前详情面板未保存的编辑提交回 raw，避免新增/重建把上一个分组的编辑冲掉。
    let before = doc.raw.clone();
    flush_open(&mut doc, &detail);
    derived::flush_open(&mut doc, &ddetail);
    aggregates::flush_open(&mut doc, &adetail);
    let renames = flush_propagate(&before, &mut doc);
    let n = {
        let Some(gs) = group_mut(&mut doc) else {
            log_err(nodes, &mut godot, "尚未载入方案");
            return;
        };
        let num = gs.len() + 1;
        gs.push(serde_json::json!({
            "id": format!("分组{num}"),
            "name": format!("分组{num}"),
        }));
        num
    };
    doc.selected_group = n as i32 - 1;
    render_all(nodes, &doc, &mut detail, &mut godot);
    for line in &renames {
        log_plain(nodes, &mut godot, line);
    }
    let is_base = n == 1;
    log_plain(
        nodes,
        &mut godot,
        &format!(
            "已新增分组（共 {n} 个）：{}",
            if is_base {
                "第 1 组 = 基础组，请补基础字段"
            } else {
                "在详情面板填覆盖（留空表达式 = 停用第 1 组字段）"
            }
        ),
    );
}

/// 顶部下拉切换了要编辑的分组（序号 = 控件当前选中项）。
pub fn on_group_picked(
    _trigger: On<GroupPicked>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut detail: bevy::prelude::NonSendMut<GroupDetail>,
    ddetail: bevy::prelude::NonSend<derived::DerivedDetail>,
    adetail: bevy::prelude::NonSend<aggregates::AggregateDetail>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = nodes_of(&refs) else { return };
    let count = group_arr(&doc).len();
    if count == 0 {
        return;
    }
    let idx = godot
        .get::<OptionButton>(nodes.group_pick)
        .get_selected()
        .max(0) as usize;
    if idx >= count {
        return;
    }
    // 切换选中前先把上一个分组详情里的未保存编辑落回 raw，切回时仍在。
    let before = doc.raw.clone();
    flush_open(&mut doc, &detail);
    derived::flush_open(&mut doc, &ddetail);
    aggregates::flush_open(&mut doc, &adetail);
    let renames = flush_propagate(&before, &mut doc);
    doc.selected_group = idx as i32;
    render_all(nodes, &doc, &mut detail, &mut godot);
    for line in &renames {
        log_plain(nodes, &mut godot, line);
    }
    let id = group_arr(&doc)
        .get(doc.selected_group as usize)
        .and_then(|g| g.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or("?")
        .to_string();
    log_plain(nodes, &mut godot, &format!("编辑分组：{id}"));
}

/// 「删除当前分组」：删除顶部下拉当前选中的分组。
pub fn on_remove_group(
    _trigger: On<RemoveCurrentGroup>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut detail: bevy::prelude::NonSendMut<GroupDetail>,
    ddetail: bevy::prelude::NonSend<derived::DerivedDetail>,
    adetail: bevy::prelude::NonSend<aggregates::AggregateDetail>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = nodes_of(&refs) else { return };
    // 先把当前详情面板未保存的编辑提交回 raw，避免删除重建时丢失。
    let before = doc.raw.clone();
    flush_open(&mut doc, &detail);
    derived::flush_open(&mut doc, &ddetail);
    aggregates::flush_open(&mut doc, &adetail);
    let renames = flush_propagate(&before, &mut doc);
    let idx = doc.selected_group.max(0) as usize;
    let was_base = idx == 0;
    let removed_id = {
        let Some(gs) = group_mut(&mut doc) else {
            return;
        };
        if idx >= gs.len() {
            return;
        }
        let rid = gs[idx]
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_string();
        gs.remove(idx);
        rid
    };
    let last = group_arr(&doc).len() as i32 - 1;
    if doc.selected_group > last {
        doc.selected_group = last.max(0);
    }
    render_all(nodes, &doc, &mut detail, &mut godot);
    for line in &renames {
        log_plain(nodes, &mut godot, line);
    }
    log_plain(nodes, &mut godot, &format!("已删除分组：{removed_id}"));
    if was_base {
        log_warn(
            nodes,
            &mut godot,
            "注意：删掉了第 1 组。若还有其它分组，新的第 1 组将没有基础字段，需重新定义",
        );
    }
}

pub fn on_add_item(
    _trigger: On<AddGroupItemClicked>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut detail: bevy::prelude::NonSendMut<GroupDetail>,
    ddetail: bevy::prelude::NonSend<derived::DerivedDetail>,
    adetail: bevy::prelude::NonSend<aggregates::AggregateDetail>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = nodes_of(&refs) else { return };
    // 先把详情面板未保存的编辑提交回 raw，避免重建行网格把已编辑行还原。
    let before = doc.raw.clone();
    flush_open(&mut doc, &detail);
    derived::flush_open(&mut doc, &ddetail);
    aggregates::flush_open(&mut doc, &adetail);
    let renames = flush_propagate(&before, &mut doc);
    let sel = doc.selected_group as usize;
    {
        let Some(gs) = group_mut(&mut doc) else {
            return;
        };
        let Some(g) = gs.get_mut(sel) else {
            log_err(nodes, &mut godot, "请先选择一个分组");
            return;
        };
        let base = is_base(sel as i32);
        let key = if base { "fields" } else { "overrides" };
        if g.get(key).and_then(|f| f.as_array()).is_none() {
            g[key] = serde_json::Value::Array(Vec::new());
        }
        let items = g
            .get_mut(key)
            .and_then(|f| f.as_array_mut())
            .expect("刚创建");
        let num = items.len() + 1;
        items.push(serde_json::json!({
            "name": if base { format!("字段{num}") } else { String::new() },
            "expr": serde_json::Value::Null
        }));
    }
    render_all(nodes, &doc, &mut detail, &mut godot);
    for line in &renames {
        log_plain(nodes, &mut godot, line);
    }
    let base = is_base(sel as i32);
    log_plain(
        nodes,
        &mut godot,
        &format!(
            "已加一行：{}",
            if base {
                "填写 字段名 + 表达式（基础字段表达式必填）"
            } else {
                "字段名填第 1 组字段；表达式留空 = 停用该字段"
            }
        ),
    );
}

pub fn on_remove_item(
    trigger: On<GroupItemRemoveClicked>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut detail: bevy::prelude::NonSendMut<GroupDetail>,
    ddetail: bevy::prelude::NonSend<derived::DerivedDetail>,
    adetail: bevy::prelude::NonSend<aggregates::AggregateDetail>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = nodes_of(&refs) else { return };
    let idx = trigger.event().index;
    // 先把详情面板未保存的编辑提交回 raw，避免删行重建把其它已编辑行还原。
    let before = doc.raw.clone();
    flush_open(&mut doc, &detail);
    derived::flush_open(&mut doc, &ddetail);
    aggregates::flush_open(&mut doc, &adetail);
    let renames = flush_propagate(&before, &mut doc);
    let sel = doc.selected_group as usize;
    let removed = {
        let Some(gs) = group_mut(&mut doc) else {
            return;
        };
        let Some(g) = gs.get_mut(sel) else { return };
        let base = is_base(sel as i32);
        let key = if base { "fields" } else { "overrides" };
        let Some(items) = g.get_mut(key).and_then(|f| f.as_array_mut()) else {
            return;
        };
        if idx < 0 || idx as usize >= items.len() {
            return;
        }
        let fname = items[idx as usize]
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_string();
        items.remove(idx as usize);
        fname
    };
    render_all(nodes, &doc, &mut detail, &mut godot);
    for line in &renames {
        log_plain(nodes, &mut godot, line);
    }
    log_plain(nodes, &mut godot, &format!("已删除字段行：{removed}"));
}

// ---------- 写回（保存 / 校验前调用） ----------

/// 当前选中分组的详情（id/name/match_sheet + 字段表）整体写回 raw 的分组项。
/// 第 1 组基础字段表达式必填；覆盖行表达式留空 = 停用（存 expr: null）。
pub fn write_back(
    raw: &mut serde_json::Value,
    detail: &GroupDetail,
    selected: i32,
) -> Result<(), String> {
    let Some(gs) = raw
        .get_mut("scheme")
        .and_then(|s| s.get_mut("groups"))
        .and_then(|s| s.as_array_mut())
    else {
        return Err("缺少 groups 数组".into());
    };
    let sel = selected as usize;
    if sel >= gs.len() {
        return Ok(()); // 无可写分组
    }
    let id = detail
        .id_edit
        .as_ref()
        .map(|e| e.get_text().to_string())
        .unwrap_or_default();
    let id = id.trim().to_string();
    if id.is_empty() {
        return Err("分组 id 不能为空".into());
    }
    let name = detail
        .name_edit
        .as_ref()
        .map(|e| e.get_text().to_string())
        .unwrap_or_default();
    let name = name.trim().to_string();
    let sheet = detail
        .sheet_edit
        .as_ref()
        .map(|e| e.get_text().to_string())
        .unwrap_or_default();
    let sheet = sheet.trim().to_string();

    let base = is_base(sel as i32);
    // 字段表校验：基础组表达式必填、字段名不重复。
    let mut seen: Vec<String> = Vec::new();
    let mut items = Vec::new();
    for j in 0..detail.item_name.len() {
        let fname = detail.item_name[j].get_text().to_string();
        let fname = fname.trim().to_string();
        if fname.is_empty() {
            continue;
        }
        if seen.iter().any(|s| s == &fname) {
            return Err(format!("分组[{id}]：字段名重复：{fname}"));
        }
        seen.push(fname.clone());
        let expr = detail.item_expr[j].get_text().to_string();
        let expr = expr.trim().to_string();
        if base && expr.is_empty() {
            return Err(format!("分组[{id}] 基础字段 [{fname}]：表达式不能为空"));
        }
        items.push((fname, expr));
    }

    // 原地更新当前分组：只覆盖本页维护的元信息与“当前列表”，
    // 保留该组另一侧数组（如基组的 overrides）与其它的既有内容。
    let g = &mut gs[sel];
    let obj = g
        .as_object_mut()
        .ok_or_else(|| format!("分组[{id}] 序列化失败"))?;
    obj.insert("id".into(), serde_json::Value::String(id));
    obj.insert("name".into(), serde_json::Value::String(name));
    if sheet.is_empty() {
        obj.remove("match_sheet");
    } else {
        obj.insert("match_sheet".into(), serde_json::Value::String(sheet));
    }
    write_items(obj, base, items);
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

fn log_warn(nodes: &EditorNodes, godot: &mut GodotAccess, line: &str) {
    info!("{line}");
    log_line(
        &mut godot.get::<RichTextLabel>(nodes.log),
        LogKind::Warn,
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
