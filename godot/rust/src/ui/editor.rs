//! 方案编辑器（v0：方案下拉 + 参数编辑/保存 + 校验）。
//! 数据源/派生/聚合/计算组/输出等区块在后续迭代加入同一骨架。

use bevy::ecs::observer::On;
use bevy::ecs::query::With;
use bevy::prelude::{Component, Query, Res, ResMut, Resource};
use godot::classes::{HBoxContainer, Label, LineEdit, OptionButton, RichTextLabel, VBoxContainer};
use godot::obj::NewAlloc;
use godot::prelude::Gd;
use godot_bevy::prelude::{GodotAccess, GodotNodeHandle, NodeTreeView};
use tracing::info;

use crate::bridge;

use super::events::{
    EditorLoadClicked, EditorSaveClicked, EditorSchemePicked, EditorValidateClicked,
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
    #[node("Margin/VBox/ParamsScroll/ParamsBox")]
    pub params_box: GodotNodeHandle,
    #[node("Margin/VBox/Log")]
    pub log: GodotNodeHandle,
}

/// 解析到的编辑器节点句柄。
#[derive(Resource, Default)]
pub struct EditorRefs {
    pub nodes: Option<EditorNodes>,
}

/// 编辑器文档状态（Send）：当前方案路径、原始 JSON、参数元信息。
#[derive(Resource, Default)]
pub struct EditorDoc {
    pub path: Option<String>,
    pub raw: Option<serde_json::Value>,
    pub param_names: Vec<String>,
    pub param_types: Vec<String>,
}

/// 动态参数行里的值输入框（Gd 非 Send → NonSend Resource）。
#[derive(Default)]
pub struct ParamRows {
    pub line_edits: Vec<Gd<LineEdit>>,
}

// ---------- 初始化：下拉 + 自动载入 ----------

/// 每帧重试：编辑器页面场景就绪后用 NodeTreeView 解析，并自动载入默认方案。
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
                let names = schemes::scheme_names();
                let mut select = godot.get::<OptionButton>(resolved.scheme_select);
                select.clear();
                for name in &names {
                    select.add_item(name);
                }
                let preferred = names.iter().position(|n| n.contains("复刻")).unwrap_or(0);
                if !names.is_empty() {
                    select.select(preferred as i32);
                }
                load_current(resolved, &mut doc, &mut rows, &mut godot);
                return;
            }
            Err(_) => continue,
        }
    }
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
    trigger: On<EditorSchemePicked>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut rows: bevy::prelude::NonSendMut<ParamRows>,
    mut godot: GodotAccess,
) {
    // 选择即载入；index 仅用于触发。
    let _ = trigger.event().index;
    if let Some(nodes) = &refs.nodes {
        load_current(nodes, &mut doc, &mut rows, &mut godot);
    }
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
    let params = raw["scheme"]["parameters"].as_array_mut();
    let Some(params) = params else {
        log(nodes, &mut godot, "方案缺少 parameters 数组");
        return;
    };
    for (i, edit) in rows.line_edits.iter().enumerate() {
        let Some(entry) = params.get_mut(i) else {
            break;
        };
        let text = edit.get_text().to_string();
        let ty = doc.param_types.get(i).map(String::as_str).unwrap_or("text");
        let value = parse_value(ty, &text);
        if let Some(value) = value {
            entry["value"] = value;
        } else {
            log(
                nodes,
                &mut godot,
                &format!(
                    "参数[{}] 值解析失败（type={ty}）: {text}",
                    doc.param_names.get(i).unwrap_or(&"?".into())
                ),
            );
            return;
        }
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

// ---------- 载入与行渲染 ----------

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
        log(nodes, godot, "方案列表为空，请先在 user://schemes 放 .json");
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

    // 清空旧的参数行
    for edit in rows.line_edits.drain(..) {
        if let Some(mut parent) = edit.get_parent() {
            parent.queue_free();
        }
    }
    rows.line_edits.clear();

    // 渲染参数行：名称 + 值输入
    let mut box_node = godot.get::<VBoxContainer>(nodes.params_box);
    for p in &params {
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
        box_node.add_child(&row);
        rows.line_edits.push(edit);
    }

    godot.get::<Label>(nodes.meta_label).set_text(&format!(
        "已载入：{meta}　参数 {n} 个，方案文件：{path_for_msg}",
        n = params.len()
    ));
    log(
        nodes,
        godot,
        &format!("载入成功：{meta}（{} 个参数）", params.len()),
    );
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
