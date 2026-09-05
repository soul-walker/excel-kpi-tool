//! 方案编辑器（v0：新建方案 + 参数增删改/保存 + 校验）。
//! 数据源/派生/聚合/计算组/输出等区块后续在同一骨架加入。

use bevy::ecs::observer::On;
use bevy::ecs::query::With;
use bevy::prelude::{Component, Query, Res, ResMut, Resource};
use godot::classes::{
    Button, HBoxContainer, Label, LineEdit, OptionButton, RichTextLabel, VBoxContainer,
};
use godot::obj::NewAlloc;
use godot::prelude::Gd;
use godot_bevy::prelude::{GodotAccess, GodotNodeHandle, GodotSignals, NodeTreeView};
use tracing::info;

use crate::bridge;

use super::events::{
    EditorAddParamClicked, EditorLoadClicked, EditorNewSchemeClicked, EditorSaveClicked,
    EditorSchemePicked, EditorValidateClicked, ParamRemoveClicked,
};
use super::schemes;

/// 编辑器页面实体标记。
#[derive(Component)]
pub struct EditorRoot;

/// scheme_editor.tscn 的类型化视图。
#[derive(NodeTreeView)]
pub struct EditorNodes {
    #[node("Margin/VBox/Toolbar/SchemeSelect")]
    pub scheme_select: GodotNodeHandle,
    #[node("Margin/VBox/MetaLabel")]
    pub meta_label: GodotNodeHandle,
    #[node("Margin/VBox/Split/ParamsPanel/ParamsVBox/ParamsScroll/ParamsBox")]
    pub params_box: GodotNodeHandle,
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
    pub path: Option<String>,
    pub raw: Option<serde_json::Value>,
    pub param_names: Vec<String>,
    pub param_types: Vec<String>,
}

/// 动态参数行（Gd 非 Send → NonSend）。
#[derive(Default)]
pub struct ParamRows {
    /// 值输入框（index 对齐 doc.param_names）。
    pub line_edits: Vec<Gd<LineEdit>>,
    /// 每行删除按钮：(参数序号, 按钮)。
    pub remove_buttons: Vec<(i32, Gd<Button>)>,
    /// 已连接的删除按钮数。
    pub connected: usize,
}

// ---------- 初始化 ----------

pub fn resolve_editor(
    roots: Query<&GodotNodeHandle, With<EditorRoot>>,
    mut refs: ResMut<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut rows: bevy::prelude::NonSendMut<ParamRows>,
    mut godot: GodotAccess,
) {
    if refs.nodes.is_some() {
        return;
    }
    for handle in &roots {
        let Some(control) = godot.try_get::<godot::classes::Control>(*handle) else {
            continue;
        };
        match EditorNodes::from_node(control) {
            Ok(nodes) => {
                info!("编辑器 NodeTreeView 解析成功");
                refs.nodes = Some(nodes);
                let Some(resolved) = &refs.nodes else {
                    return;
                };
                populate_scheme_select(resolved, &mut godot, Some("复刻"));
                load_current(resolved, &mut doc, &mut rows, &mut godot);
                return;
            }
            Err(_) => continue,
        }
    }
}

/// 动态删除按钮的 typed 连接（每帧补连未连接的按钮）。
pub fn connect_remove_buttons(
    mut rows: bevy::prelude::NonSendMut<ParamRows>,
    signals: GodotSignals<ParamRemoveClicked>,
) {
    while rows.connected < rows.remove_buttons.len() {
        let (index, button) = rows.remove_buttons[rows.connected].clone();
        let handle = GodotNodeHandle::from_instance_id(button.instance_id());
        signals.connect(handle, "pressed", None, move |_a, _n, _e| {
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

// ---------- 事件处理 ----------

pub fn on_load_clicked(
    _trigger: On<EditorLoadClicked>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut rows: bevy::prelude::NonSendMut<ParamRows>,
    mut godot: GodotAccess,
) {
    if let Some(nodes) = &refs.nodes {
        load_current(nodes, &mut doc, &mut rows, &mut godot);
    }
}

pub fn on_scheme_picked(
    _trigger: On<EditorSchemePicked>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut rows: bevy::prelude::NonSendMut<ParamRows>,
    mut godot: GodotAccess,
) {
    if let Some(nodes) = &refs.nodes {
        load_current(nodes, &mut doc, &mut rows, &mut godot);
    }
}

pub fn on_new_scheme_clicked(
    _trigger: On<EditorNewSchemeClicked>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut rows: bevy::prelude::NonSendMut<ParamRows>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = &refs.nodes else { return };
    // 新建：生成一份最小合法方案，刷新下拉并载入。
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let name = format!("新方案-{stamp}.json");
    let path = schemes::scheme_path(&name);
    let body = serde_json::json!({
        "schema_version": 1,
        "scheme": {
            "name": "新方案",
            "description": "通过 GUI 新建",
            "entity": { "key_field": "姓名", "duplicate": "report" },
            "parameters": [],
            "sources": [],
            "derived": [],
            "aggregates": [],
            "groups": [],
            "outputs": []
        }
    });
    match serde_json::to_string_pretty(&body)
        .ok()
        .map(|text| std::fs::write(&path, text))
    {
        Some(Ok(())) => log(nodes, &mut godot, &format!("已新建：{path}")),
        _ => {
            log(nodes, &mut godot, "新建方案失败（写入错误）");
            return;
        }
    }
    populate_scheme_select(nodes, &mut godot, Some(name.as_str()));
    load_current(nodes, &mut doc, &mut rows, &mut godot);
}

pub fn on_add_param_clicked(
    _trigger: On<EditorAddParamClicked>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut rows: bevy::prelude::NonSendMut<ParamRows>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = &refs.nodes else { return };
    let Some(raw) = &mut doc.raw else {
        log(nodes, &mut godot, "尚未载入方案");
        return;
    };
    let Some(params) = raw["scheme"]["parameters"].as_array_mut() else {
        log(nodes, &mut godot, "方案缺少 parameters 数组");
        return;
    };
    let idx = params.len();
    let pname = format!("参数{idx}");
    params.push(serde_json::json!({
        "name": pname, "label": pname, "value_type": "text", "value": ""
    }));
    doc.param_names.push(pname);
    doc.param_types.push("text".into());
    render_param_rows(nodes, &doc, &mut rows, &mut godot);
    log(
        nodes,
        &mut godot,
        &format!("已新增参数（共 {} 个）", idx + 1),
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
    let Some(raw) = &mut doc.raw else { return };
    let Some(params) = raw["scheme"]["parameters"].as_array_mut() else {
        return;
    };
    if idx < 0 || idx as usize >= params.len() {
        return;
    }
    let removed = params.remove(idx as usize).to_string();
    doc.param_names.remove(idx as usize);
    doc.param_types.remove(idx as usize);
    render_param_rows(nodes, &doc, &mut rows, &mut godot);
    log(nodes, &mut godot, &format!("已删除参数 [{idx}]：{removed}"));
}

pub fn on_validate_clicked(
    _trigger: On<EditorValidateClicked>,
    refs: Res<EditorRefs>,
    doc: Res<EditorDoc>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = &refs.nodes else { return };
    let Some(path) = &doc.path else {
        log(nodes, &mut godot, "尚未载入方案");
        return;
    };
    let json = bridge::validate_scheme_json(path);
    log(nodes, &mut godot, &bridge::render_result(&json));
}

pub fn on_save_clicked(
    _trigger: On<EditorSaveClicked>,
    refs: Res<EditorRefs>,
    doc: Res<EditorDoc>,
    rows: bevy::prelude::NonSend<ParamRows>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = &refs.nodes else { return };
    let (Some(path), Some(mut raw)) = (&doc.path, doc.raw.clone()) else {
        log(nodes, &mut godot, "尚未载入方案");
        return;
    };
    let Some(params) = raw["scheme"]["parameters"].as_array_mut() else {
        log(nodes, &mut godot, "方案缺少 parameters 数组");
        return;
    };
    for (i, edit) in rows.line_edits.iter().enumerate() {
        let Some(entry) = params.get_mut(i) else {
            break;
        };
        let text = edit.get_text().to_string();
        let ty = doc.param_types.get(i).map(String::as_str).unwrap_or("text");
        let Some(value) = parse_value(ty, &text) else {
            log(
                nodes,
                &mut godot,
                &format!(
                    "参数[{}] 值解析失败（type={ty}）: {text}",
                    doc.param_names.get(i).unwrap_or(&"?".into())
                ),
            );
            return;
        };
        entry["value"] = value;
    }
    if let Ok(text) = serde_json::to_string_pretty(&raw) {
        match std::fs::write(path, text) {
            Ok(()) => log(nodes, &mut godot, &format!("已保存：{path}")),
            Err(e) => log(nodes, &mut godot, &format!("保存失败：{e}")),
        }
    } else {
        log(nodes, &mut godot, "序列化方案失败");
    }
}

// ---------- 载入 / 渲染 ----------

fn load_current(
    nodes: &EditorNodes,
    doc: &mut EditorDoc,
    rows: &mut ParamRows,
    godot: &mut GodotAccess,
) {
    let name = godot
        .get::<OptionButton>(nodes.scheme_select)
        .get_text()
        .to_string();
    if name.is_empty() {
        log(
            nodes,
            godot,
            "方案列表为空，请先在 user://schemes 放 .json 或新建",
        );
        return;
    }
    let path = schemes::scheme_path(&name);
    let raw_text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            log(nodes, godot, &format!("读取失败：{e}"));
            return;
        }
    };
    let raw: serde_json::Value = match serde_json::from_str(&raw_text) {
        Ok(v) => v,
        Err(e) => {
            log(nodes, godot, &format!("JSON 解析失败：{e}"));
            return;
        }
    };
    let scheme_node = raw.get("scheme");
    let meta = scheme_node
        .and_then(|s| s.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("（无名方案）")
        .to_string();
    let params = scheme_node
        .and_then(|s| s.get("parameters"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let path_for_msg = path.clone();

    doc.path = Some(path);
    doc.raw = Some(raw);
    doc.param_names.clear();
    doc.param_types.clear();
    for p in &params {
        doc.param_names.push(
            p.get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("?")
                .to_string(),
        );
        doc.param_types.push(
            p.get("value_type")
                .and_then(|v| v.as_str())
                .unwrap_or("text")
                .to_string(),
        );
    }
    godot.get::<Label>(nodes.meta_label).set_text(&format!(
        "已载入：{meta}　参数 {} 个　方案文件：{path_for_msg}",
        params.len()
    ));
    render_param_rows(nodes, doc, rows, godot);
    log(
        nodes,
        godot,
        &format!("载入成功：{meta}（{} 个参数）", params.len()),
    );
}

/// 依据 doc 重新渲染参数行（先释放旧行）。
fn render_param_rows(
    nodes: &EditorNodes,
    doc: &EditorDoc,
    rows: &mut ParamRows,
    godot: &mut GodotAccess,
) {
    for edit in rows.line_edits.drain(..) {
        if let Some(mut parent) = edit.get_parent() {
            parent.queue_free();
        }
    }
    rows.line_edits.clear();
    rows.remove_buttons.clear();
    rows.connected = 0;

    let params = doc
        .raw
        .as_ref()
        .and_then(|r| r.get("scheme"))
        .and_then(|s| s.get("parameters"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut box_node = godot.get::<VBoxContainer>(nodes.params_box);
    for (i, p) in params.iter().enumerate() {
        let pname = p.get("name").and_then(|v| v.as_str()).unwrap_or("?");
        let ptype = p
            .get("value_type")
            .and_then(|v| v.as_str())
            .unwrap_or("text");
        let value = p.get("value").map(json_display).unwrap_or_default();

        let mut row = HBoxContainer::new_alloc();
        let mut name_label = Label::new_alloc();
        name_label.set_text(&format!("{pname} ({ptype})"));
        row.add_child(&name_label);

        let mut edit = LineEdit::new_alloc();
        edit.set_text(&value);
        row.add_child(&edit);

        let mut remove = Button::new_alloc();
        remove.set_text("删除");
        row.add_child(&remove);

        box_node.add_child(&row);
        rows.line_edits.push(edit);
        rows.remove_buttons.push((i as i32, remove));
    }
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
    info!("{line}");
    let mut log = godot.get::<RichTextLabel>(nodes.log);
    log.append_text(&format!("{line}\n"));
}
