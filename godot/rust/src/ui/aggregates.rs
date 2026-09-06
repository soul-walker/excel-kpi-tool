//! 聚合指标编辑页签。
//!
//! 顶部下拉（AggPick）列出全部聚合指标，选中即切换详情面板；无列表行按钮。
//! 详情面板编辑选中指标的全部属性（模型即 raw["scheme"]["aggregates"] 数组项）：
//! - 指标 id（表达式里以 `聚合.该id` 引用）；
//! - 来源表 from（下拉，数据源/派生表 id）——切换后分组列候选随新来源刷新；
//! - 分组列 group_by（下拉，from 表的列，通常为主键列如 姓名）；
//! - 行过滤 where（可选，CodeEdit，带补全）；
//! - 聚合表达式 expr（CodeEdit，带补全，行内列写 `{from}.列`，可用 SUM/COUNT/AVG…、
//!   引用前序指标 `聚合.x` 与 `参数.x`）。
//!
//! 每个指标对应 SQL：`FROM {from} WHERE {where} GROUP BY {group_by} SELECT {expr}`。
//! 详情控件全部为静态节点，每次重建只重填内容；任何会重建面板的操作
//! （新增/删除/切换指标、切换 from）都先把当前面板内容写回它所属的指标项
//! （`flush_open`），因此编辑不会因重建丢失；改名（指标 id 等）随后经
//! `refs::propagate` 全方案同步。

use bevy::ecs::observer::On;
use bevy::prelude::{Res, ResMut};
use godot::classes::{CodeEdit, Label, LineEdit, OptionButton, RichTextLabel};
use godot::prelude::Gd;
use godot_bevy::prelude::{GodotAccess, GodotNodeHandle, GodotSignals};
use tracing::info;

use super::derived;
use super::editor::{EditorDoc, EditorNodes, EditorRefs};
use super::events::{
    AddAggregateClicked, AggFromChanged, AggregatePicked, ExprCompletionRequested,
    ExprFocusEntered, ExprFocusExited, ExprTextChanged, NameFieldFocusLost, RemoveCurrentAggregate,
};
use super::expr;
use super::refs;
use super::view::{LogKind, log_line};

/// group_by 下拉里“未选分组列”的占位项文本（flush 时映射回空字符串）。
const GROUP_NONE: &str = "（选列分组…）";
/// from 为空/来源表暂不可解析时的提示项。
const GROUP_NO_SOURCE: &str = "（先在上方选来源表）";
/// from 表存在但没有任何列时的提示项。
const GROUP_NO_COLS: &str = "（该表暂无可分组的列）";
/// from 下拉里“还没有任何来源表”的提示项。
const FROM_NONE: &str = "（暂无来源表）";

/// 当前选中聚合指标的详情控件（Gd 非 Send → NonSend）。
#[derive(Default)]
pub struct AggregateDetail {
    /// 指标 id 输入。
    pub id_edit: Option<Gd<LineEdit>>,
    /// from 下拉。
    pub from_btn: Option<Gd<OptionButton>>,
    /// group_by 下拉。
    pub group_btn: Option<Gd<OptionButton>>,
    /// 行过滤 where 输入（CodeEdit，带补全）。
    pub where_edit: Option<Gd<CodeEdit>>,
    /// 聚合表达式输入（CodeEdit，带补全）。
    pub expr_edit: Option<Gd<CodeEdit>>,
    /// 详情面板当前展示的指标序号（None = 暂无指标/尚未渲染）。
    /// 新增/删除/切换/改 from 前，据此把未保存编辑写回对应指标项。
    pub rendered_for: Option<i32>,
    /// 指标 id 静态输入框已连失焦的实例（改名即同步）。
    pub id_focus_connected: Option<GodotNodeHandle>,
    /// where/expr 静态 CodeEdit 已连补全信号的实例（跨重绘复用，避免重复连接）。
    pub where_connected: Option<GodotNodeHandle>,
    pub expr_connected: Option<GodotNodeHandle>,
}

impl AggregateDetail {
    pub fn clear(&mut self) {
        self.id_edit = None;
        self.from_btn = None;
        self.group_btn = None;
        self.where_edit = None;
        self.expr_edit = None;
        self.rendered_for = None;
        self.id_focus_connected = None;
        // 注意：where_connected / expr_connected 不清——静态 CodeEdit 节点跨重绘复用。
    }
}

/// 全部候选表 id：数据源 id + 派生 id（复用派生页同一份规则，见 derived.rs）。
fn table_candidates(doc: &EditorDoc) -> Vec<String> {
    derived::table_candidates(doc)
}

fn agg_arr(doc: &EditorDoc) -> Vec<serde_json::Value> {
    doc.raw
        .as_ref()
        .and_then(|r| r.get("scheme"))
        .and_then(|s| s.get("aggregates"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
}

fn agg_mut(doc: &mut EditorDoc) -> Option<&mut Vec<serde_json::Value>> {
    doc.raw
        .as_mut()
        .and_then(|r| r.get_mut("scheme"))
        .and_then(|s| s.get_mut("aggregates"))
        .and_then(|v| v.as_array_mut())
}

fn nodes_of(refs: &EditorRefs) -> Option<&EditorNodes> {
    refs.nodes.as_ref()
}

/// 方案 entity 的 key_field（如 姓名），供新建指标预填 group_by。
fn entity_key(raw: Option<&serde_json::Value>) -> String {
    raw.and_then(|r| r.get("scheme"))
        .and_then(|s| s.get("entity"))
        .and_then(|e| e.get("key_field"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

/// 重建聚合切换下拉与详情面板（载入/新增/删除/切换时调用）。
pub fn render_all(
    nodes: &EditorNodes,
    doc: &EditorDoc,
    detail: &mut AggregateDetail,
    godot: &mut GodotAccess,
) {
    render_picker(nodes, doc, godot);
    render_detail(nodes, doc, detail, godot);
}

/// 把全部指标填进顶部下拉并选中当前项（空列表时禁用，显示占位文案）。
fn render_picker(nodes: &EditorNodes, doc: &EditorDoc, godot: &mut GodotAccess) {
    let items = agg_arr(doc);
    let mut pick = godot.get::<OptionButton>(nodes.agg_pick);
    pick.clear();
    pick.set_tooltip_text("当前编辑的指标；切换后详情面板即载入它");
    if items.is_empty() {
        pick.set_disabled(true);
        pick.add_item("暂无指标：点「+ 新增指标」");
        pick.select(0);
        return;
    }
    pick.set_disabled(false);
    for (i, a) in items.iter().enumerate() {
        let id = a.get("id").and_then(|v| v.as_str()).unwrap_or("?");
        pick.add_item(&format!("{}. {}", i + 1, id));
    }
    let sel = (doc.selected_aggregate as usize).min(items.len() - 1);
    pick.select(sel as i32);
}

/// 重填详情面板内容（静态控件，仅换文本/候选；不含动态子节点）。
fn render_detail(
    nodes: &EditorNodes,
    doc: &EditorDoc,
    detail: &mut AggregateDetail,
    godot: &mut GodotAccess,
) {
    detail.clear();
    let items = agg_arr(doc);
    let sel = if items.is_empty() {
        0
    } else {
        (doc.selected_aggregate as usize).min(items.len() - 1)
    };
    detail.rendered_for = if items.is_empty() {
        None
    } else {
        Some(sel as i32)
    };

    let mut title = godot.get::<Label>(nodes.agg_detail_title);
    let mut id_edit = godot.get::<LineEdit>(nodes.agg_id_edit);
    let mut from_btn = godot.get::<OptionButton>(nodes.agg_from_select);
    let mut group_btn = godot.get::<OptionButton>(nodes.agg_group_select);
    let mut where_edit = godot.get::<CodeEdit>(nodes.agg_where_edit);
    let mut expr_edit = godot.get::<CodeEdit>(nodes.agg_expr_edit);
    expr::configure_code_edit(
        &mut where_edit,
        "留空=不过滤；引用带前缀，如 qingjia.类型 != '公'",
        0.0,
        true,
        doc.raw.as_ref(),
    );
    expr::configure_code_edit(
        &mut expr_edit,
        "按组求值，如 SUM(qingjia.天数) / 聚合.请假超总天数 >= 参数.请假超过天数",
        0.0,
        true,
        doc.raw.as_ref(),
    );

    let Some(a) = items.get(sel) else {
        title.set_text("指标详情（先新增指标）");
        id_edit.set_text("");
        where_edit.set_text("");
        expr_edit.set_text("");
        from_btn.clear();
        from_btn.set_disabled(true);
        from_btn.add_item(FROM_NONE);
        from_btn.select(0);
        group_btn.clear();
        group_btn.set_disabled(true);
        group_btn.add_item(GROUP_NO_SOURCE);
        group_btn.select(0);
        return;
    };

    let id = a.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let from = a.get("from").and_then(|v| v.as_str()).unwrap_or("");
    let stored_group = a.get("group_by").and_then(|v| v.as_str()).unwrap_or("");
    let where_ = a.get("where").and_then(|v| v.as_str()).unwrap_or("");
    let expr = a.get("expr").and_then(|v| v.as_str()).unwrap_or("");

    title.set_text(&format!(
        "指标详情 · {id}（第 {} 个 / 共 {} 个）",
        sel + 1,
        items.len()
    ));
    id_edit.set_text(id);

    // from 下拉：候选不存在时补一项再选中；空值且没有候选 → 禁用的占位项。
    let candidates = table_candidates(doc);
    from_btn.clear();
    if from.is_empty() && candidates.is_empty() {
        from_btn.set_disabled(true);
        from_btn.add_item(FROM_NONE);
        from_btn.select(0);
    } else {
        from_btn.set_disabled(false);
        for c in &candidates {
            from_btn.add_item(c);
        }
        if !from.is_empty() && !candidates.iter().any(|c| c == from) {
            from_btn.add_item(from);
        }
        let pick_index = from_btn.get_item_count() - 1;
        if from.is_empty() {
            from_btn.select(0); // 未选来源时默认第一张表（仅展示，保存随 flush 生效）
        } else {
            let mut found = None;
            for j in 0..from_btn.get_item_count() {
                if from_btn.get_item_text(j).to_string() == from {
                    found = Some(j);
                    break;
                }
            }
            from_btn.select(found.unwrap_or(pick_index));
        }
    }

    // group_by：按“当前展示的 from”取列候选（与详情面板所见一致）。
    let live_from = from_btn.get_text().to_string().trim().to_string();
    let cols = doc
        .raw
        .as_ref()
        .and_then(|r| expr::table_columns(r, &live_from));
    fill_group_options(&mut group_btn, cols.as_ref(), stored_group);

    where_edit.set_text(where_);
    expr_edit.set_text(expr);

    detail.id_edit = Some(id_edit);
    detail.from_btn = Some(from_btn);
    detail.group_btn = Some(group_btn);
    detail.where_edit = Some(where_edit);
    detail.expr_edit = Some(expr_edit);
}

/// 依据“from 表列候选”重填 group_by 下拉。
/// - 候选缺失（from 空/表不存在）→ 禁用，展示占位（仍有存量值时保留其文本）；
/// - 候选存在 → 「（选列分组…）」+ 列名；存量值不在候选里时补一项保留。
fn fill_group_options(btn: &mut Gd<OptionButton>, cols: Option<&Vec<String>>, stored: &str) {
    btn.clear();
    let stored = stored.trim();
    let has_stored = !stored.is_empty();
    match cols {
        Some(cols) if !cols.is_empty() => {
            btn.set_disabled(false);
            btn.add_item(GROUP_NONE);
            let mut chosen = 0i32; // 默认「未选」
            for (j, c) in cols.iter().enumerate() {
                btn.add_item(c);
                if c == stored {
                    chosen = j as i32 + 1;
                }
            }
            if has_stored && !cols.iter().any(|c| c == stored) {
                btn.add_item(stored);
                chosen = btn.get_item_count() - 1;
            }
            btn.select(chosen);
        }
        Some(_) => {
            // 表存在但没有列。
            btn.set_disabled(true);
            if has_stored {
                btn.add_item(stored);
            } else {
                btn.add_item(GROUP_NO_COLS);
            }
            btn.select(0);
        }
        None => {
            // from 空 / 找不到表。
            btn.set_disabled(true);
            if has_stored {
                btn.add_item(stored);
            } else {
                btn.add_item(GROUP_NO_SOURCE);
            }
            btn.select(0);
        }
    }
}

// ---------- 动态信号连接 ----------

/// 每帧补连：指标 id 输入框失焦（改名即同步）与 where/expr 两个静态 CodeEdit 的
/// 补全信号（同一实例只连一次；页面重建换新实例后才重连）。
pub fn connect_aggregates_signals(
    mut detail: bevy::prelude::NonSendMut<AggregateDetail>,
    text_sig: GodotSignals<ExprTextChanged>,
    comp_sig: GodotSignals<ExprCompletionRequested>,
    enter_sig: GodotSignals<ExprFocusEntered>,
    exit_sig: GodotSignals<ExprFocusExited>,
    name_focus: GodotSignals<NameFieldFocusLost>,
) {
    if let Some(edit) = detail.id_edit.clone() {
        let handle = GodotNodeHandle::new(edit);
        if detail.id_focus_connected != Some(handle) {
            name_focus.connect(handle, "focus_exited", None, move |_a, n, _e| {
                Some(NameFieldFocusLost { node: n })
            });
            detail.id_focus_connected = Some(handle);
        }
    }
    if let Some(edit) = detail.where_edit.clone() {
        let handle = GodotNodeHandle::new(edit);
        if detail.where_connected != Some(handle) {
            expr::connect_code_edit_signals(&text_sig, &comp_sig, &enter_sig, &exit_sig, handle);
            detail.where_connected = Some(handle);
        }
    }
    if let Some(edit) = detail.expr_edit.clone() {
        let handle = GodotNodeHandle::new(edit);
        if detail.expr_connected != Some(handle) {
            expr::connect_code_edit_signals(&text_sig, &comp_sig, &enter_sig, &exit_sig, handle);
            detail.expr_connected = Some(handle);
        }
    }
}

// ---------- 增删/切换（观察者） ----------

/// 增删/切换/改 from 前的“落库”：把详情面板当前内容合并写回 raw 的“正在展示”指标项。
/// 与 write_back 相同的字段归一（空 where 不出现在 JSON），但不做校验——
/// 保存/校验时仍会校验，这里只为不丢编辑。
fn flush_current(raw: &mut serde_json::Value, detail: &AggregateDetail) {
    let Some(idx) = detail.rendered_for else {
        return;
    };
    let Some(items) = raw
        .get_mut("scheme")
        .and_then(|s| s.get_mut("aggregates"))
        .and_then(|s| s.as_array_mut())
    else {
        return;
    };
    let Some(a) = items.get_mut(idx as usize) else {
        return;
    };
    let Some(obj) = a.as_object_mut() else {
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
    let from = if from.trim() == FROM_NONE {
        String::new()
    } else {
        from.trim().to_string()
    };
    let group_label = detail
        .group_btn
        .as_ref()
        .map(|b| b.get_text().to_string())
        .unwrap_or_default();
    let group_by = group_value_of(&group_label);
    let where_ = detail
        .where_edit
        .as_ref()
        .map(|e| e.get_text().to_string())
        .unwrap_or_default();
    let where_ = where_.trim().to_string();
    let expr = detail
        .expr_edit
        .as_ref()
        .map(|e| e.get_text().to_string())
        .unwrap_or_default();
    let expr = expr.trim().to_string();

    obj.insert("id".into(), serde_json::Value::String(id));
    obj.insert("from".into(), serde_json::Value::String(from));
    obj.insert("group_by".into(), serde_json::Value::String(group_by));
    obj.insert("expr".into(), serde_json::Value::String(expr));
    if where_.is_empty() {
        obj.remove("where");
    } else {
        obj.insert("where".into(), serde_json::Value::String(where_));
    }
}

/// group_by 下拉文本 → 模型值：占位提示项映射回空字符串。
fn group_value_of(label: &str) -> String {
    match label {
        GROUP_NONE | GROUP_NO_SOURCE | GROUP_NO_COLS => String::new(),
        other => other.trim().to_string(),
    }
}

/// 增删/切换指标或改 from 前调用：先落库再改动模型并重建页面。
pub(crate) fn flush_open(doc: &mut EditorDoc, detail: &AggregateDetail) {
    let Some(raw) = doc.raw.as_mut() else {
        return;
    };
    flush_current(raw, detail);
}

/// 若刚写入的指标详情含改名（指标 id），同步全方案引用并返回日志。
/// 应在 flush_open 之后、模型增删之前调用（数量未变，检测才可靠）。
fn flush_propagate(before: &Option<serde_json::Value>, doc: &mut EditorDoc) -> Vec<String> {
    match (before.as_ref(), doc.raw.as_mut()) {
        (Some(b), Some(a)) => refs::propagate(b, a),
        _ => Vec::new(),
    }
}

pub fn on_add_aggregate(
    _trigger: On<AddAggregateClicked>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut detail: bevy::prelude::NonSendMut<AggregateDetail>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = nodes_of(&refs) else { return };
    // 先把当前详情面板未保存的编辑提交回 raw，避免新增/重建把上一个指标的编辑冲掉。
    let before = doc.raw.clone();
    flush_open(&mut doc, &detail);
    let renames = flush_propagate(&before, &mut doc);
    let key_field = entity_key(doc.raw.as_ref());
    let candidates = table_candidates(&doc);
    let default_from = candidates.first().cloned().unwrap_or_default();
    // 新指标默认按 entity.key_field（如 姓名）分组——来源表含该列时预填。
    let default_group = if !default_from.is_empty() && !key_field.is_empty() {
        doc.raw
            .as_ref()
            .and_then(|r| expr::table_columns(r, &default_from))
            .map(|cols| {
                if cols.iter().any(|c| c == &key_field) {
                    key_field.clone()
                } else {
                    String::new()
                }
            })
            .unwrap_or_default()
    } else {
        String::new()
    };
    let n = {
        let Some(items) = agg_mut(&mut doc) else {
            log_err(nodes, &mut godot, "尚未载入方案");
            return;
        };
        items.push(serde_json::json!({
            "id": format!("指标{}", items.len() + 1),
            "from": default_from,
            "group_by": default_group,
            "expr": "",
        }));
        items.len()
    };
    doc.selected_aggregate = n as i32 - 1;
    render_all(nodes, &doc, &mut detail, &mut godot);
    for line in &renames {
        log_plain(nodes, &mut godot, line);
    }
    log_plain(
        nodes,
        &mut godot,
        &format!("已新增指标（共 {n} 个）：在详情面板编辑 id/来源/分组列/表达式"),
    );
}

/// 顶部下拉切换了要编辑的指标（序号 = 控件当前选中项）。
pub fn on_agg_picked(
    _trigger: On<AggregatePicked>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut detail: bevy::prelude::NonSendMut<AggregateDetail>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = nodes_of(&refs) else { return };
    let count = agg_arr(&doc).len();
    if count == 0 {
        return;
    }
    let idx = godot
        .get::<OptionButton>(nodes.agg_pick)
        .get_selected()
        .max(0) as usize;
    if idx >= count {
        return;
    }
    // 切换选中前先把上一个指标详情里的未保存编辑落回 raw，切回时仍在。
    let before = doc.raw.clone();
    flush_open(&mut doc, &detail);
    let renames = flush_propagate(&before, &mut doc);
    doc.selected_aggregate = idx as i32;
    render_all(nodes, &doc, &mut detail, &mut godot);
    for line in &renames {
        log_plain(nodes, &mut godot, line);
    }
    let id = agg_arr(&doc)
        .get(doc.selected_aggregate as usize)
        .and_then(|a| a.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or("?")
        .to_string();
    log_plain(nodes, &mut godot, &format!("编辑指标：{id}"));
}

/// 「删除当前指标」：删除顶部下拉当前选中的指标。
pub fn on_remove_current(
    _trigger: On<RemoveCurrentAggregate>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut detail: bevy::prelude::NonSendMut<AggregateDetail>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = nodes_of(&refs) else { return };
    // 先把当前详情面板未保存的编辑提交回 raw，避免删除重建时丢失。
    let before = doc.raw.clone();
    flush_open(&mut doc, &detail);
    let renames = flush_propagate(&before, &mut doc);
    let idx = doc.selected_aggregate.max(0) as usize;
    let removed_id = {
        let Some(items) = agg_mut(&mut doc) else {
            return;
        };
        if idx >= items.len() {
            return;
        }
        let rid = items[idx]
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_string();
        items.remove(idx);
        rid
    };
    let last = agg_arr(&doc).len() as i32 - 1;
    if doc.selected_aggregate > last {
        doc.selected_aggregate = last.max(0);
    }
    render_all(nodes, &doc, &mut detail, &mut godot);
    for line in &renames {
        log_plain(nodes, &mut godot, line);
    }
    log_plain(nodes, &mut godot, &format!("已删除指标：{removed_id}"));
}

/// 详情面板里“来源表 from”被改动：先落库再重建面板，使分组列候选随新来源刷新。
pub fn on_agg_from_changed(
    _trigger: On<AggFromChanged>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut detail: bevy::prelude::NonSendMut<AggregateDetail>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = nodes_of(&refs) else { return };
    if doc.selected_aggregate < 0 {
        return;
    }
    let before = doc.raw.clone();
    flush_open(&mut doc, &detail);
    let renames = flush_propagate(&before, &mut doc);
    render_all(nodes, &doc, &mut detail, &mut godot);
    for line in &renames {
        log_plain(nodes, &mut godot, line);
    }
    let from = godot
        .get::<OptionButton>(nodes.agg_from_select)
        .get_text()
        .to_string();
    log_plain(
        nodes,
        &mut godot,
        &format!("指标来源表已切换：{from}（分组列选项已刷新）"),
    );
}

// ---------- 写回（保存 / 校验前调用） ----------

/// 当前选中指标的详情（id/from/group_by/where/expr）整体写回 raw 的指标项。
/// from/group_by/expr 的合法性不在本层校验：改名传播（refs::propagate）先按
/// “新旧 id 对照”把引用改掉，随后由 refs::check_table_refs 与引擎校验统一查。
pub fn write_back(
    raw: &mut serde_json::Value,
    detail: &AggregateDetail,
    selected: i32,
) -> Result<(), String> {
    let Some(items) = raw
        .get_mut("scheme")
        .and_then(|s| s.get_mut("aggregates"))
        .and_then(|s| s.as_array_mut())
    else {
        return Err("缺少 aggregates 数组".into());
    };
    let sel = selected as usize;
    if sel >= items.len() {
        return Ok(()); // 无可写指标
    }
    let id = detail
        .id_edit
        .as_ref()
        .map(|e| e.get_text().to_string())
        .unwrap_or_default();
    let id = id.trim().to_string();
    if id.is_empty() {
        return Err("指标 id 不能为空".into());
    }
    let from = detail
        .from_btn
        .as_ref()
        .map(|b| b.get_text().to_string())
        .unwrap_or_default();
    let from = if from.trim() == FROM_NONE {
        String::new()
    } else {
        from.trim().to_string()
    };
    let group_label = detail
        .group_btn
        .as_ref()
        .map(|b| b.get_text().to_string())
        .unwrap_or_default();
    let group_by = group_value_of(&group_label);
    let where_ = detail
        .where_edit
        .as_ref()
        .map(|e| e.get_text().to_string())
        .unwrap_or_default();
    let where_ = where_.trim().to_string();
    let expr = detail
        .expr_edit
        .as_ref()
        .map(|e| e.get_text().to_string())
        .unwrap_or_default();
    let expr = expr.trim().to_string();

    let mut a = serde_json::json!({
        "id": id,
        "from": from,
        "group_by": group_by,
        "expr": expr,
    });
    if !where_.is_empty() {
        a["where"] = serde_json::Value::String(where_);
    }
    items[sel] = a;
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
