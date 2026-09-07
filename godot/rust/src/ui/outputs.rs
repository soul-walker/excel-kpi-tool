//! 输出定义编辑页签。
//!
//! 顶部下拉（OutputPick）列出全部输出（raw["scheme"]["outputs"] 数组项），选中即切换详情面板。
//! 每个输出模型为 `{ id, kind: { patch_template: {...} | new_file: {...} } }`：
//! - `patch_template`（回写模板）：file / apply(inplace|copy) / backup / dry_run / row_key /
//!   groups（分组 → sheet 映射）/ columns（字段 → 模板表头列，含数字格式与空值行为）；
//! - `new_file`（生成新文件）：file / from（数据源或派生表）/ sheet / columns（表头 → 字段）。
//!
//! 详情面板 = 静态骨架（id、kind、两套按 kind 切换显隐的分节）+ 动态行网格：
//! sheet 映射行（分组下拉 + sheet 名）、回写列行（字段下拉 + 表头 + 格式 + 空值）、
//! 新文件列行（表头 + 字段下拉）。下拉项文本直接等于模型值（无歧义）；
//! 空值用首项「（选择…）」占位表达，flush 时按选项表把选中项映射回值。
//!
//! 重建时机与派生/分组页一致：任何会重建面板的操作（新增/删除/切换输出、改 kind/from、
//! 加删 sheet/列行）都先 `flush_open` 把当前面板内容写回它所属的输出项，编辑不会丢失。
//! 输出 id 不被方案其它部分引用，故不参与“改名即同步”；分组 id / 基础字段名等改名
//! 对输出的同步由 editor 层在 refs 传播后重建本页完成。

use bevy::ecs::observer::On;
use bevy::prelude::{Res, ResMut};
use godot::builtin::{Color, Vector2};
use godot::classes::control::SizeFlags;
use godot::classes::{
    Button, CheckButton, GridContainer, Label, LineEdit, OptionButton, RichTextLabel, VBoxContainer,
};
use godot::obj::NewAlloc;
use godot::prelude::Gd;
use godot_bevy::prelude::{GodotAccess, GodotNodeHandle, GodotSignals};
use tracing::info;

use super::derived;
use super::editor::{EditorDoc, EditorNodes, EditorRefs};
use super::events::{
    AddOutputClicked, AddOutputColClicked, AddOutputNfColClicked, AddOutputSheetClicked,
    NameFieldFocusLost, OutputColRemoveClicked, OutputFromChanged, OutputKindChanged,
    OutputNfColRemoveClicked, OutputPicked, OutputSheetRemoveClicked, RemoveCurrentOutput,
};
use super::refs;
use super::view::{LogKind, log_line};

/// 输出方式下拉（标签含引擎 key）。
const KIND_LABELS: [(&str, &str); 2] = [
    ("patch_template", "回写模板（patch_template）"),
    ("new_file", "生成新文件（new_file）"),
];

/// 回写策略下拉（引擎 key → 显示标签）。
const APPLY_KEYS: [&str; 2] = ["inplace", "copy"];
const APPLY_LABELS: [&str; 2] = ["inplace：写回原文件", "copy：另存结果副本"];

/// 空值行为下拉（引擎 key → 显示标签）。
const BLANK_KEYS: [&str; 3] = ["blank", "zero", "keep"];
const BLANK_LABELS: [&str; 3] = ["blank：留空", "zero：写 0", "keep：保留原值"];

const KIND_PATCH: &str = "patch_template";
const KIND_NEW_FILE: &str = "new_file";

/// 下拉首项“未选/选择占位”文本（flush 时按选项表把索引 0 映射回空值）。
const GROUP_NONE: &str = "（选择分组…）";
const FIELD_NONE: &str = "（选择字段…）";
const COL_NONE: &str = "（选择列…）";
const FROM_NONE: &str = "（选择来源表…）";

/// 当前选中输出的详情控件（Gd 非 Send → NonSend）。
#[derive(Default)]
pub struct OutputDetail {
    /// 输出 id 输入（静态）。
    pub id_edit: Option<Gd<LineEdit>>,
    /// 输出方式下拉（静态）。
    pub kind_btn: Option<Gd<OptionButton>>,
    // ---- patch_template 分节（PatchBox）----
    pub pt_file: Option<Gd<LineEdit>>,
    pub pt_apply: Option<Gd<OptionButton>>,
    pub pt_backup: Option<Gd<CheckButton>>,
    pub pt_dry: Option<Gd<CheckButton>>,
    pub pt_key: Option<Gd<LineEdit>>,
    /// sheet 映射行：分组下拉 / sheet 名输入。
    pub sheet_group_btn: Vec<Gd<OptionButton>>,
    pub sheet_name_edit: Vec<Gd<LineEdit>>,
    pub sheet_del: Vec<(i32, Gd<Button>)>,
    pub sheet_del_connected: usize,
    /// 每行分组下拉的“第 2 项起”候选值（不含首项占位）。
    pub sheet_group_opts: Vec<Vec<String>>,
    /// 回写列行：字段下拉 / 表头名 / 数字格式 / 空值行为。
    pub pt_field_btn: Vec<Gd<OptionButton>>,
    pub pt_header_edit: Vec<Gd<LineEdit>>,
    pub pt_fmt_edit: Vec<Gd<LineEdit>>,
    pub pt_blank_btn: Vec<Gd<OptionButton>>,
    pub pt_col_del: Vec<(i32, Gd<Button>)>,
    pub pt_col_del_connected: usize,
    pub pt_field_opts: Vec<Vec<String>>,
    // ---- new_file 分节（NfBox）----
    pub nf_file: Option<Gd<LineEdit>>,
    pub nf_from_btn: Option<Gd<OptionButton>>,
    /// from 下拉候选（第 2 项起；首项为占位）。
    pub nf_from_opts: Vec<String>,
    pub nf_sheet: Option<Gd<LineEdit>>,
    /// 新文件列行：表头名输入 / 字段下拉。
    pub nf_header_edit: Vec<Gd<LineEdit>>,
    pub nf_field_btn: Vec<Gd<OptionButton>>,
    pub nf_col_del: Vec<(i32, Gd<Button>)>,
    pub nf_col_del_connected: usize,
    pub nf_field_opts: Vec<Vec<String>>,
    /// 详情面板当前展示的输出序号（None = 暂无输出/尚未渲染）。
    /// 增删/切换/改 kind/from 前，据此把未保存编辑写回对应输出项。
    pub rendered_for: Option<i32>,
    /// 当前面板按哪种方式渲染（patch_template / new_file；与 raw 中该输出的 kind 一致）。
    pub rendered_kind: Option<String>,
    /// 输出 id 输入已连失焦实例（经 editor 的 NameFieldFocusLost 统一处理）。
    pub id_focus_connected: Option<GodotNodeHandle>,
}

impl OutputDetail {
    pub fn clear(&mut self) {
        self.id_edit = None;
        self.kind_btn = None;
        self.pt_file = None;
        self.pt_apply = None;
        self.pt_backup = None;
        self.pt_dry = None;
        self.pt_key = None;
        self.sheet_group_btn.clear();
        self.sheet_name_edit.clear();
        self.sheet_del.clear();
        self.sheet_del_connected = 0;
        self.sheet_group_opts.clear();
        self.pt_field_btn.clear();
        self.pt_header_edit.clear();
        self.pt_fmt_edit.clear();
        self.pt_blank_btn.clear();
        self.pt_col_del.clear();
        self.pt_col_del_connected = 0;
        self.pt_field_opts.clear();
        self.nf_file = None;
        self.nf_from_btn = None;
        self.nf_from_opts.clear();
        self.nf_sheet = None;
        self.nf_header_edit.clear();
        self.nf_field_btn.clear();
        self.nf_col_del.clear();
        self.nf_col_del_connected = 0;
        self.nf_field_opts.clear();
        self.rendered_for = None;
        self.rendered_kind = None;
        self.id_focus_connected = None;
    }
}

fn outputs_arr(doc: &EditorDoc) -> Vec<serde_json::Value> {
    doc.raw
        .as_ref()
        .and_then(|r| r.get("scheme"))
        .and_then(|s| s.get("outputs"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
}

fn outputs_mut(doc: &mut EditorDoc) -> Option<&mut Vec<serde_json::Value>> {
    let scheme = doc.raw.as_mut()?.get_mut("scheme")?;
    if scheme.get("outputs").is_none() {
        scheme["outputs"] = serde_json::Value::Array(Vec::new());
    }
    scheme.get_mut("outputs")?.as_array_mut()
}

fn nodes_of(refs: &EditorRefs) -> Option<&EditorNodes> {
    refs.nodes.as_ref()
}

/// 输出项里的 kind 键：`patch_template` / `new_file`（对象里仅一个键）。
fn kind_key_of(obj: &serde_json::Value) -> String {
    obj.get("kind")
        .and_then(|k| k.as_object())
        .and_then(|m| {
            m.keys()
                .next()
                .cloned()
                .filter(|k| *k == KIND_PATCH || *k == KIND_NEW_FILE)
        })
        .unwrap_or_else(|| KIND_PATCH.to_string())
}

fn kind_key_at(idx: i32) -> &'static str {
    KIND_LABELS
        .get(idx as usize)
        .map_or(KIND_PATCH, |(k, _)| *k)
}

fn kind_index_of(key: &str) -> i32 {
    KIND_LABELS.iter().position(|(k, _)| *k == key).unwrap_or(0) as i32
}

fn clear_children(box_node: &mut VBoxContainer) {
    let child_count = box_node.get_child_count();
    for i in (0..child_count).rev() {
        if let Some(mut child) = box_node.get_child(i) {
            child.queue_free();
        }
    }
}

fn gray() -> Color {
    Color::from_rgb(0.62, 0.62, 0.62)
}

fn hint_label(name: &str, text: &str) -> Gd<Label> {
    let mut hint = Label::new_alloc();
    hint.set_name(name);
    hint.set_text(text);
    hint.set_modulate(Color::from_rgb(0.6, 0.6, 0.6));
    hint
}

/// 网格表头单元格（灰度小字）。
fn head_cell(grid: &mut GridContainer, text: &str, min_w: Option<f32>, expand: bool) {
    let mut head = Label::new_alloc();
    head.set_text(text);
    head.set_modulate(gray());
    if let Some(w) = min_w {
        head.set_custom_minimum_size(Vector2::new(w, 0.0));
    }
    if expand {
        head.set_h_size_flags(SizeFlags::EXPAND_FILL);
    }
    grid.add_child(&head);
}

/// 下拉选中值 → 模型值：首项占位 → 空串；否则查选项表。
fn btn_value(btn: &Gd<OptionButton>, opts: &[String]) -> String {
    let idx = btn.get_selected();
    if idx <= 0 {
        return String::new();
    }
    opts.get(idx as usize - 1).cloned().unwrap_or_default()
}

/// 重建输出切换下拉与详情面板（载入/新增/删除/切换时调用）。
pub fn render_all(
    nodes: &EditorNodes,
    doc: &EditorDoc,
    detail: &mut OutputDetail,
    godot: &mut GodotAccess,
) {
    render_picker(nodes, doc, godot);
    render_detail(nodes, doc, detail, godot);
}

/// 把全部输出填进顶部下拉并选中当前项（空列表时禁用，显示占位文案）。
fn render_picker(nodes: &EditorNodes, doc: &EditorDoc, godot: &mut GodotAccess) {
    let outs = outputs_arr(doc);
    let mut pick = godot.get::<OptionButton>(nodes.output_pick);
    pick.clear();
    pick.set_tooltip_text("当前编辑的输出；切换后详情面板即载入它");
    if outs.is_empty() {
        pick.set_disabled(true);
        pick.add_item("暂无输出：点「+ 新增输出」");
        pick.select(0);
        return;
    }
    pick.set_disabled(false);
    for (i, o) in outs.iter().enumerate() {
        let id = o.get("id").and_then(|v| v.as_str()).unwrap_or("?");
        let marker = if kind_key_of(o) == KIND_NEW_FILE {
            "（新文件）"
        } else {
            "（回写模板）"
        };
        pick.add_item(&format!("{}. {id} {marker}", i + 1));
    }
    let sel = (doc.selected_output as usize).min(outs.len() - 1);
    pick.select(sel as i32);
}

/// 依据 raw["scheme"]["groups"] 计算“可写入的分组字段全集”：
/// 第 1 组基础字段 + 后续组 overrides 中出现的新增/覆盖字段名（保持出现顺序）。
fn patch_field_candidates(doc: &EditorDoc) -> Vec<String> {
    let Some(gs) = doc
        .raw
        .as_ref()
        .and_then(|r| r.get("scheme"))
        .and_then(|s| s.get("groups"))
        .and_then(|v| v.as_array())
    else {
        return Vec::new();
    };
    let mut names: Vec<String> = Vec::new();
    for g in gs {
        let rows = g
            .get("fields")
            .and_then(|v| v.as_array())
            .or_else(|| g.get("overrides").and_then(|v| v.as_array()))
            .cloned()
            .unwrap_or_default();
        for f in &rows {
            let name = f.get("name").and_then(|v| v.as_str()).unwrap_or("");
            if !name.is_empty() && !names.iter().any(|n| n == name) {
                names.push(name.to_string());
            }
        }
    }
    names
}

/// 新文件列字段候选：来源表列 alias（数据源）或派生视图列名；
/// 派生未定义视图列（原样透传）时取它 from 表的列（递归一层）。
fn new_file_field_candidates(doc: &EditorDoc, from: &str) -> Vec<String> {
    let Some(scheme) = doc.raw.as_ref().and_then(|r| r.get("scheme")) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    if let Some(srcs) = scheme.get("sources").and_then(|v| v.as_array())
        && let Some(src) = srcs
            .iter()
            .find(|s| s.get("id").and_then(|v| v.as_str()) == Some(from))
    {
        if let Some(cols) = src.get("columns").and_then(|v| v.as_array()) {
            out.extend(
                cols.iter()
                    .filter_map(|c| c.get("alias").and_then(|v| v.as_str()))
                    .map(String::from),
            );
        }
        return out;
    }
    if let Some(ds) = scheme.get("derived").and_then(|v| v.as_array())
        && let Some(d) = ds
            .iter()
            .find(|d| d.get("id").and_then(|v| v.as_str()) == Some(from))
    {
        if let Some(cols) = d.get("columns").and_then(|v| v.as_array()) {
            out.extend(
                cols.iter()
                    .filter_map(|c| c.get("name").and_then(|v| v.as_str()))
                    .map(String::from),
            );
        }
        if out.is_empty() {
            let inner_from = d.get("from").and_then(|v| v.as_str()).unwrap_or("");
            if !inner_from.is_empty() {
                return new_file_field_candidates(doc, inner_from);
            }
        }
    }
    out
}

/// 把候选 + 存量值填进“带首项占位”的下拉；返回“第 2 项起”的候选表（与控件一一对应）。
fn fill_select(
    btn: &mut Gd<OptionButton>,
    placeholder: &str,
    candidates: &[String],
    stored: &str,
) -> Vec<String> {
    btn.clear();
    btn.set_disabled(false);
    btn.add_item(placeholder);
    let mut opts: Vec<String> = candidates.to_vec();
    for c in candidates {
        btn.add_item(c);
    }
    let mut chosen = 0i32; // 空值占位
    let stored = stored.trim();
    if !stored.is_empty() {
        match candidates.iter().position(|c| c == stored) {
            Some(j) => chosen = j as i32 + 1,
            None => {
                btn.add_item(stored);
                opts.push(stored.to_string());
                chosen = btn.get_item_count() - 1;
            }
        }
    }
    btn.select(chosen);
    opts
}

/// 当前选中输出的详情整体重建（含按 kind 切换两套静态分节的显隐与动态行网格）。
fn render_detail(
    nodes: &EditorNodes,
    doc: &EditorDoc,
    detail: &mut OutputDetail,
    godot: &mut GodotAccess,
) {
    detail.clear();
    let outs = outputs_arr(doc);
    let sel = if outs.is_empty() {
        0
    } else {
        (doc.selected_output as usize).min(outs.len() - 1)
    };
    detail.rendered_for = if outs.is_empty() {
        None
    } else {
        Some(sel as i32)
    };

    let mut title = godot.get::<Label>(nodes.output_detail_title);
    let mut id_edit = godot.get::<LineEdit>(nodes.output_id_edit);
    let mut kind_btn = godot.get::<OptionButton>(nodes.output_kind_select);
    kind_btn.clear();
    for (_, label) in KIND_LABELS {
        kind_btn.add_item(label);
    }
    let mut patch_box = godot.get::<VBoxContainer>(nodes.patch_box);
    let mut nf_box = godot.get::<VBoxContainer>(nodes.nf_box);
    patch_box.set_visible(false);
    nf_box.set_visible(false);
    clear_children(&mut godot.get::<VBoxContainer>(nodes.sheet_grid_box));
    clear_children(&mut godot.get::<VBoxContainer>(nodes.col_grid_box));
    clear_children(&mut godot.get::<VBoxContainer>(nodes.nf_col_grid_box));

    let Some(obj) = outs.get(sel) else {
        title.set_text("输出详情（先新增输出）");
        id_edit.set_text("");
        kind_btn.select(0);
        return;
    };

    let id = obj.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let kind = kind_key_of(obj);
    title.set_text(&format!(
        "输出详情 · {id}（第 {} 个 / 共 {} 个）· {}",
        sel + 1,
        outs.len(),
        if kind == KIND_NEW_FILE {
            "生成新文件"
        } else {
            "回写模板"
        }
    ));
    id_edit.set_text(id);
    kind_btn.select(kind_index_of(&kind));
    detail.id_edit = Some(id_edit);
    detail.kind_btn = Some(kind_btn);
    detail.rendered_kind = Some(kind.clone());

    if kind == KIND_PATCH {
        patch_box.set_visible(true);
        fill_patch_section(nodes, doc, detail, obj, godot);
    } else {
        nf_box.set_visible(true);
        fill_new_file_section(nodes, doc, detail, obj, godot);
    }
}

/// 回写模板分节：file/apply/备份/dry-run/key + sheet 映射 + 写入列。
fn fill_patch_section(
    nodes: &EditorNodes,
    doc: &EditorDoc,
    detail: &mut OutputDetail,
    obj: &serde_json::Value,
    godot: &mut GodotAccess,
) {
    let p = obj.get("kind").and_then(|k| k.get(KIND_PATCH));
    let text_of = |v: Option<&serde_json::Value>| -> String {
        v.and_then(|x| x.as_str()).unwrap_or("").to_string()
    };
    let file = text_of(p.and_then(|x| x.get("file")));
    let apply = text_of(p.and_then(|x| x.get("apply")));
    let apply = if APPLY_KEYS.contains(&apply.as_str()) {
        apply
    } else {
        "inplace".to_string()
    };
    let backup = p
        .and_then(|x| x.get("backup"))
        .and_then(|x| x.as_bool())
        .unwrap_or(true);
    let dry = p
        .and_then(|x| x.get("dry_run"))
        .and_then(|x| x.as_bool())
        .unwrap_or(true);
    let key = text_of(p.and_then(|x| x.get("row_key")));
    let groups = p
        .and_then(|x| x.get("groups"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let cols = p
        .and_then(|x| x.get("columns"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut pt_file = godot.get::<LineEdit>(nodes.patch_file_edit);
    pt_file.set_text(&file);
    detail.pt_file = Some(pt_file);

    let mut pt_apply = godot.get::<OptionButton>(nodes.patch_apply_select);
    pt_apply.clear();
    for label in APPLY_LABELS {
        pt_apply.add_item(label);
    }
    pt_apply.select(APPLY_KEYS.iter().position(|k| *k == apply).unwrap_or(0) as i32);
    detail.pt_apply = Some(pt_apply);

    let mut backup_btn = godot.get::<CheckButton>(nodes.patch_backup_btn);
    backup_btn.set_pressed(backup);
    detail.pt_backup = Some(backup_btn);
    let mut dry_btn = godot.get::<CheckButton>(nodes.patch_dry_btn);
    dry_btn.set_pressed(dry);
    detail.pt_dry = Some(dry_btn);

    let mut key_edit = godot.get::<LineEdit>(nodes.patch_key_edit);
    key_edit.set_text(&key);
    detail.pt_key = Some(key_edit);

    // —— sheet 映射 ——
    let group_ids: Vec<String> = {
        let mut v = Vec::new();
        if let Some(gs) = doc
            .raw
            .as_ref()
            .and_then(|r| r.get("scheme"))
            .and_then(|s| s.get("groups"))
            .and_then(|x| x.as_array())
        {
            v.extend(
                gs.iter()
                    .filter_map(|g| g.get("id").and_then(|x| x.as_str()))
                    .map(String::from),
            );
        }
        v
    };
    let mut sheet_hint = godot.get::<Label>(nodes.patch_sheet_hint);
    let mut add_sheet_btn = godot.get::<Button>(nodes.add_sheet_btn);
    sheet_hint.set_tooltip_text(
        "分组 → sheet 映射：各分组人的计算结果写回模板的哪些工作表。\n先到「计算分组」页定义分组；每行 = 一个 分组 + 模板 sheet 名，同一分组可重复映射多个 sheet。",
    );
    if group_ids.is_empty() {
        sheet_hint.set_text("尚未定义计算分组：先到「计算分组」页添加");
        add_sheet_btn.set_disabled(true);
    } else {
        sheet_hint.set_text(&format!(
            "已定义 {} 个分组：给每个分组映射一个模板 sheet",
            group_ids.len()
        ));
        add_sheet_btn.set_disabled(false);
    }
    let mut sheet_box = godot.get::<VBoxContainer>(nodes.sheet_grid_box);
    clear_children(&mut sheet_box);
    let mut grid = GridContainer::new_alloc();
    grid.set_name("OutSheetGrid");
    grid.set_columns(3);
    grid.add_theme_constant_override("h_separation", 6);
    grid.add_theme_constant_override("v_separation", 2);
    head_cell(&mut grid, "分组", Some(160.0), false);
    head_cell(&mut grid, "模板 sheet 名", None, true);
    head_cell(&mut grid, "", None, false);
    if groups.is_empty() {
        let hint = hint_label("SheetEmptyHint", "无分组映射：点「+ 新增 sheet」添加");
        grid.add_child(&hint);
    }
    for g in &groups {
        let stored_group = g.get("group").and_then(|v| v.as_str()).unwrap_or("");
        let stored_sheet = g.get("sheet").and_then(|v| v.as_str()).unwrap_or("");
        let mut g_btn = OptionButton::new_alloc();
        g_btn.set_custom_minimum_size(Vector2::new(160.0, 0.0));
        g_btn.set_tooltip_text("输出到模板前按哪个计算分组算值");
        let opts = fill_select(&mut g_btn, GROUP_NONE, &group_ids, stored_group);
        let mut s_edit = LineEdit::new_alloc();
        s_edit.set_text(stored_sheet);
        s_edit.set_placeholder("如 非校聘");
        s_edit.set_h_size_flags(SizeFlags::EXPAND_FILL);
        let idx = detail.sheet_del.len() as i32;
        let mut del = Button::new_alloc();
        del.set_text("删除");
        grid.add_child(&g_btn);
        grid.add_child(&s_edit);
        grid.add_child(&del);
        detail.sheet_group_btn.push(g_btn);
        detail.sheet_name_edit.push(s_edit);
        detail.sheet_del.push((idx, del));
        detail.sheet_group_opts.push(opts);
    }
    sheet_box.add_child(&grid);

    // —— 写入列 ——
    let field_ids = patch_field_candidates(doc);
    let mut col_hint = godot.get::<Label>(nodes.patch_col_hint);
    let mut add_col_btn = godot.get::<Button>(nodes.add_col_btn);
    col_hint.set_tooltip_text(
        "写入列 = 字段 → 模板表头列：运行时按 字段 取该分组下此字段的计算值，按 模板表头名 在模板里定位列写入。\n数字格式如 0 / 0.00（留空 = 不指定）；空值行为 = 值缺失时留空 / 写 0 / 保留原值。需要先有分组字段才能选字段。",
    );
    if field_ids.is_empty() {
        col_hint.set_text("尚未定义计算字段：先到「计算分组」页添加基础字段");
        add_col_btn.set_disabled(true);
    } else {
        col_hint.set_text(&format!(
            "已定义 {} 个可写字段：逐行选字段并填模板表头名",
            field_ids.len()
        ));
        add_col_btn.set_disabled(false);
    }
    let mut col_box = godot.get::<VBoxContainer>(nodes.col_grid_box);
    clear_children(&mut col_box);
    let mut grid = GridContainer::new_alloc();
    grid.set_name("OutColGrid");
    grid.set_columns(5);
    grid.add_theme_constant_override("h_separation", 6);
    grid.add_theme_constant_override("v_separation", 2);
    head_cell(&mut grid, "字段", Some(150.0), false);
    head_cell(&mut grid, "模板表头名", None, true);
    head_cell(&mut grid, "数字格式", Some(76.0), false);
    head_cell(&mut grid, "空值", Some(130.0), false);
    head_cell(&mut grid, "", None, false);
    if cols.is_empty() {
        let hint = hint_label("ColEmptyHint", "无写入列：点「+ 新增写入列」添加");
        grid.add_child(&hint);
    }
    for c in &cols {
        let field = c.get("field").and_then(|v| v.as_str()).unwrap_or("");
        let header = c.get("header").and_then(|v| v.as_str()).unwrap_or("");
        let fmt = c.get("num_fmt").and_then(|v| v.as_str()).unwrap_or("");
        let blank = c.get("blank").and_then(|v| v.as_str()).unwrap_or("blank");
        let mut f_btn = OptionButton::new_alloc();
        f_btn.set_custom_minimum_size(Vector2::new(150.0, 0.0));
        f_btn.set_tooltip_text("要写入的计算字段（取该分组下此字段的计算值）");
        let opts = fill_select(&mut f_btn, FIELD_NONE, &field_ids, field);
        let mut h_edit = LineEdit::new_alloc();
        h_edit.set_text(header);
        h_edit.set_placeholder("模板表头名（按表头定位列）");
        h_edit.set_h_size_flags(SizeFlags::EXPAND_FILL);
        let mut n_edit = LineEdit::new_alloc();
        n_edit.set_text(fmt);
        n_edit.set_placeholder("0.00");
        n_edit.set_custom_minimum_size(Vector2::new(76.0, 0.0));
        n_edit.set_tooltip_text("数字格式，如 0 / 0.00；留空 = 不指定");
        let mut b_btn = OptionButton::new_alloc();
        b_btn.set_custom_minimum_size(Vector2::new(130.0, 0.0));
        for label in BLANK_LABELS {
            b_btn.add_item(label);
        }
        b_btn.select(BLANK_KEYS.iter().position(|k| *k == blank).unwrap_or(0) as i32);
        let idx = detail.pt_col_del.len() as i32;
        let mut del = Button::new_alloc();
        del.set_text("删除");
        grid.add_child(&f_btn);
        grid.add_child(&h_edit);
        grid.add_child(&n_edit);
        grid.add_child(&b_btn);
        grid.add_child(&del);
        detail.pt_field_btn.push(f_btn);
        detail.pt_header_edit.push(h_edit);
        detail.pt_fmt_edit.push(n_edit);
        detail.pt_blank_btn.push(b_btn);
        detail.pt_col_del.push((idx, del));
        detail.pt_field_opts.push(opts);
    }
    col_box.add_child(&grid);
}

/// 生成新文件分节：file/from/sheet + 输出列（表头名 + 来源字段）。
fn fill_new_file_section(
    nodes: &EditorNodes,
    doc: &EditorDoc,
    detail: &mut OutputDetail,
    obj: &serde_json::Value,
    godot: &mut GodotAccess,
) {
    let nf = obj.get("kind").and_then(|k| k.get(KIND_NEW_FILE));
    let text_of = |v: Option<&serde_json::Value>| -> String {
        v.and_then(|x| x.as_str()).unwrap_or("").to_string()
    };
    let file = text_of(nf.and_then(|x| x.get("file")));
    let from = text_of(nf.and_then(|x| x.get("from")));
    let sheet = text_of(nf.and_then(|x| x.get("sheet")));
    let cols = nf
        .and_then(|x| x.get("columns"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut nf_file = godot.get::<LineEdit>(nodes.nf_file_edit);
    nf_file.set_text(&file);
    detail.nf_file = Some(nf_file);

    let candidates = derived::table_candidates(doc);
    let mut from_btn = godot.get::<OptionButton>(nodes.nf_from_select);
    if candidates.is_empty() {
        from_btn.clear();
        from_btn.set_disabled(true);
        from_btn.add_item("（暂无可选来源表：先定义数据源/派生）");
        from_btn.select(0);
        detail.nf_from_btn = Some(from_btn);
        detail.nf_from_opts = Vec::new();
    } else {
        let opts = fill_select(&mut from_btn, FROM_NONE, &candidates, &from);
        from_btn.set_disabled(false);
        detail.nf_from_btn = Some(from_btn);
        detail.nf_from_opts = opts;
    }

    let mut nf_sheet = godot.get::<LineEdit>(nodes.nf_sheet_edit);
    nf_sheet.set_text(&sheet);
    detail.nf_sheet = Some(nf_sheet);

    // —— 输出列 ——
    let live_from = {
        let mut live = String::new();
        if let Some(btn) = &detail.nf_from_btn {
            live = btn_value(btn, &detail.nf_from_opts);
        }
        live
    };
    let field_ids = if live_from.is_empty() {
        Vec::new()
    } else {
        new_file_field_candidates(doc, &live_from)
    };
    let mut col_hint = godot.get::<Label>(nodes.nf_col_hint);
    let mut add_col_btn = godot.get::<Button>(nodes.add_nf_col_btn);
    col_hint.set_tooltip_text(
        "输出列 = 新文件里的 表头名 → 字段：先在上方选「数据来源 from」，再逐行填表头名并选该表的列（数据源列 alias / 派生视图列名）。新文件会包含这些表头与数据。",
    );
    if live_from.is_empty() {
        col_hint.set_text("先在上方选「数据来源 from」（数据源或派生表）");
        add_col_btn.set_disabled(true);
    } else if field_ids.is_empty() {
        col_hint.set_text(&format!("来源 {live_from} 暂无已知列，无法添加输出列"));
        add_col_btn.set_disabled(true);
    } else {
        col_hint.set_text(&format!(
            "来源 {live_from}：逐行填表头名并选字段（共 {} 个可选）",
            field_ids.len()
        ));
        add_col_btn.set_disabled(false);
    }
    let mut col_box = godot.get::<VBoxContainer>(nodes.nf_col_grid_box);
    clear_children(&mut col_box);
    let mut grid = GridContainer::new_alloc();
    grid.set_name("OutNfColGrid");
    grid.set_columns(3);
    grid.add_theme_constant_override("h_separation", 6);
    grid.add_theme_constant_override("v_separation", 2);
    head_cell(&mut grid, "表头名", Some(180.0), false);
    head_cell(&mut grid, "字段", None, true);
    head_cell(&mut grid, "", None, false);
    if cols.is_empty() {
        let hint = hint_label("NfColEmptyHint", "无输出列：点「+ 新增列」添加");
        grid.add_child(&hint);
    }
    for c in &cols {
        let header = c.get("header").and_then(|v| v.as_str()).unwrap_or("");
        let field = c.get("field").and_then(|v| v.as_str()).unwrap_or("");
        let mut h_edit = LineEdit::new_alloc();
        h_edit.set_text(header);
        h_edit.set_placeholder("新文件里的表头名");
        h_edit.set_custom_minimum_size(Vector2::new(180.0, 0.0));
        let mut f_btn = OptionButton::new_alloc();
        f_btn.set_h_size_flags(SizeFlags::EXPAND_FILL);
        f_btn.set_tooltip_text("来源表的列 alias / 派生视图列名");
        let opts = fill_select(&mut f_btn, COL_NONE, &field_ids, field);
        let idx = detail.nf_col_del.len() as i32;
        let mut del = Button::new_alloc();
        del.set_text("删除");
        grid.add_child(&h_edit);
        grid.add_child(&f_btn);
        grid.add_child(&del);
        detail.nf_header_edit.push(h_edit);
        detail.nf_field_btn.push(f_btn);
        detail.nf_col_del.push((idx, del));
        detail.nf_field_opts.push(opts);
    }
    col_box.add_child(&grid);
}

// ---------- 动态信号连接 ----------

/// 每帧补连新渲染的动态行：三类删除按钮 + 输出 id 输入框失焦。
pub fn connect_outputs_signals(
    mut detail: bevy::prelude::NonSendMut<OutputDetail>,
    sheet_del_sig: GodotSignals<OutputSheetRemoveClicked>,
    col_del_sig: GodotSignals<OutputColRemoveClicked>,
    nf_del_sig: GodotSignals<OutputNfColRemoveClicked>,
    name_focus: GodotSignals<NameFieldFocusLost>,
) {
    while detail.sheet_del_connected < detail.sheet_del.len() {
        let (index, button) = detail.sheet_del[detail.sheet_del_connected].clone();
        let handle = GodotNodeHandle::from_instance_id(button.instance_id());
        sheet_del_sig.connect(handle, "pressed", None, move |_a, _n, _e| {
            Some(OutputSheetRemoveClicked { index })
        });
        detail.sheet_del_connected += 1;
    }
    while detail.pt_col_del_connected < detail.pt_col_del.len() {
        let (index, button) = detail.pt_col_del[detail.pt_col_del_connected].clone();
        let handle = GodotNodeHandle::from_instance_id(button.instance_id());
        col_del_sig.connect(handle, "pressed", None, move |_a, _n, _e| {
            Some(OutputColRemoveClicked { index })
        });
        detail.pt_col_del_connected += 1;
    }
    while detail.nf_col_del_connected < detail.nf_col_del.len() {
        let (index, button) = detail.nf_col_del[detail.nf_col_del_connected].clone();
        let handle = GodotNodeHandle::from_instance_id(button.instance_id());
        nf_del_sig.connect(handle, "pressed", None, move |_a, _n, _e| {
            Some(OutputNfColRemoveClicked { index })
        });
        detail.nf_col_del_connected += 1;
    }
    // 输出 id 失焦：输出 id 不被方案引用，但统一走 editor 的失焦落库流程（改名日志通常为空）。
    if let Some(edit) = detail.id_edit.clone() {
        let handle = GodotNodeHandle::new(edit);
        if detail.id_focus_connected != Some(handle) {
            name_focus.connect(handle, "focus_exited", None, move |_a, n, _e| {
                Some(NameFieldFocusLost { node: n })
            });
            detail.id_focus_connected = Some(handle);
        }
    }
}

// ---------- 增删/切换（观察者） ----------

/// 增删/切换输出、改 kind/from、加删行前的“落库”：把详情面板当前内容写回
/// raw 的“正在展示”输出项。与 write_back 相同的字段归一，但不做校验——
/// 保存/校验时仍会校验，这里只为不丢编辑。
fn flush_current(raw: &mut serde_json::Value, detail: &OutputDetail) {
    let Some(idx) = detail.rendered_for else {
        return;
    };
    let Some(kind) = detail.rendered_kind.as_deref() else {
        return;
    };
    let Some(outs) = raw
        .get_mut("scheme")
        .and_then(|s| s.get_mut("outputs"))
        .and_then(|v| v.as_array_mut())
    else {
        return;
    };
    let Some(obj) = outs.get_mut(idx as usize) else {
        return;
    };
    let id = detail
        .id_edit
        .as_ref()
        .map(|e| e.get_text().to_string())
        .unwrap_or_default();
    let obj = obj.as_object_mut();
    let Some(obj) = obj else {
        return;
    };
    obj.insert(
        "id".into(),
        serde_json::Value::String(id.trim().to_string()),
    );
    let payload = if kind == KIND_PATCH {
        gather_patch_payload(detail)
    } else {
        gather_new_file_payload(detail)
    };
    obj.insert("kind".into(), serde_json::Value::Object(payload));
}

/// 把当前面板控件内容（patch_template）组装成 kind 对象（key → payload Map）。
fn gather_patch_payload(detail: &OutputDetail) -> serde_json::Map<String, serde_json::Value> {
    let mut p = serde_json::Map::new();
    let file = detail
        .pt_file
        .as_ref()
        .map(|e| e.get_text().to_string())
        .unwrap_or_default();
    p.insert(
        "file".into(),
        serde_json::Value::String(file.trim().to_string()),
    );
    let apply = detail
        .pt_apply
        .as_ref()
        .map(|b| {
            let i = b.get_selected();
            APPLY_KEYS
                .get(i as usize)
                .copied()
                .unwrap_or("inplace")
                .to_string()
        })
        .unwrap_or_else(|| "inplace".to_string());
    p.insert("apply".into(), serde_json::Value::String(apply));
    let backup = detail
        .pt_backup
        .as_ref()
        .map(|b| b.is_pressed())
        .unwrap_or(true);
    p.insert("backup".into(), serde_json::Value::Bool(backup));
    let dry = detail
        .pt_dry
        .as_ref()
        .map(|b| b.is_pressed())
        .unwrap_or(true);
    p.insert("dry_run".into(), serde_json::Value::Bool(dry));
    let key = detail
        .pt_key
        .as_ref()
        .map(|e| e.get_text().to_string())
        .unwrap_or_default();
    let key = key.trim().to_string();
    if !key.is_empty() {
        p.insert("row_key".into(), serde_json::Value::String(key));
    }
    // groups：跳过整行为空的行（分组/ sheet 都空）；保留部分填写行待校验提示。
    let mut groups = Vec::new();
    let n = detail.sheet_group_btn.len();
    for j in 0..n {
        let group = detail.sheet_group_btn[j].get_selected();
        let opts = detail.sheet_group_opts.get(j).cloned().unwrap_or_default();
        let group_val = if group <= 0 {
            String::new()
        } else {
            opts.get(group as usize - 1).cloned().unwrap_or_default()
        };
        let sheet = detail.sheet_name_edit[j].get_text().to_string();
        let sheet = sheet.trim().to_string();
        if group_val.is_empty() && sheet.is_empty() {
            continue;
        }
        groups.push(serde_json::json!({
            "group": group_val,
            "sheet": sheet,
        }));
    }
    if !groups.is_empty() {
        p.insert("groups".into(), serde_json::Value::Array(groups));
    }
    // columns：字段与表头均空的行跳过；其余保留（部分填写待校验）。
    let mut cols = Vec::new();
    let n = detail.pt_field_btn.len();
    for j in 0..n {
        let idx = detail.pt_field_btn[j].get_selected();
        let opts = detail.pt_field_opts.get(j).cloned().unwrap_or_default();
        let field = if idx <= 0 {
            String::new()
        } else {
            opts.get(idx as usize - 1).cloned().unwrap_or_default()
        };
        let header = detail.pt_header_edit[j].get_text().to_string();
        let header = header.trim().to_string();
        if field.is_empty() && header.is_empty() {
            continue;
        }
        let mut c = serde_json::json!({ "field": field, "header": header });
        let fmt = detail.pt_fmt_edit[j].get_text().to_string();
        let fmt = fmt.trim().to_string();
        if !fmt.is_empty() {
            c["num_fmt"] = serde_json::Value::String(fmt);
        }
        let bi = detail.pt_blank_btn[j].get_selected();
        let blank = BLANK_KEYS.get(bi as usize).copied().unwrap_or("blank");
        if blank != "blank" {
            c["blank"] = serde_json::Value::String(blank.to_string());
        }
        cols.push(c);
    }
    if !cols.is_empty() {
        p.insert("columns".into(), serde_json::Value::Array(cols));
    }
    serde_json::Map::from_iter([(KIND_PATCH.to_string(), serde_json::Value::Object(p))])
}

/// 把当前面板控件内容（new_file）组装成 kind 对象。
fn gather_new_file_payload(detail: &OutputDetail) -> serde_json::Map<String, serde_json::Value> {
    let file = detail
        .nf_file
        .as_ref()
        .map(|e| e.get_text().to_string())
        .unwrap_or_default();
    let from = detail
        .nf_from_btn
        .as_ref()
        .map(|b| btn_value(b, &detail.nf_from_opts))
        .unwrap_or_default();
    let sheet = detail
        .nf_sheet
        .as_ref()
        .map(|e| e.get_text().to_string())
        .unwrap_or_default();
    let mut p = serde_json::Map::new();
    p.insert(
        "file".into(),
        serde_json::Value::String(file.trim().to_string()),
    );
    p.insert(
        "from".into(),
        serde_json::Value::String(from.trim().to_string()),
    );
    p.insert(
        "sheet".into(),
        serde_json::Value::String(sheet.trim().to_string()),
    );
    let mut cols = Vec::new();
    let n = detail.nf_field_btn.len();
    for j in 0..n {
        let idx = detail.nf_field_btn[j].get_selected();
        let opts = detail.nf_field_opts.get(j).cloned().unwrap_or_default();
        let field = if idx <= 0 {
            String::new()
        } else {
            opts.get(idx as usize - 1).cloned().unwrap_or_default()
        };
        let header = detail.nf_header_edit[j].get_text().to_string();
        let header = header.trim().to_string();
        if field.is_empty() && header.is_empty() {
            continue;
        }
        cols.push(serde_json::json!({ "header": header, "field": field }));
    }
    if !cols.is_empty() {
        p.insert("columns".into(), serde_json::Value::Array(cols));
    }
    serde_json::Map::from_iter([(KIND_NEW_FILE.to_string(), serde_json::Value::Object(p))])
}

/// 增删/切换/改 kind/from 前调用：先落库再改动模型并重建页面。
pub(crate) fn flush_open(doc: &mut EditorDoc, detail: &OutputDetail) {
    let Some(raw) = doc.raw.as_mut() else {
        return;
    };
    flush_current(raw, detail);
}

/// 若刚写入的输出详情含改名，同步全方案引用并返回日志。输出本身不产改名，
/// 保留此入口以便与派生/分组页结构一致（空实现返回无日志）。
fn flush_propagate(before: &Option<serde_json::Value>, doc: &mut EditorDoc) -> Vec<String> {
    match (before.as_ref(), doc.raw.as_mut()) {
        (Some(b), Some(a)) => refs::propagate(b, a),
        _ => Vec::new(),
    }
}

pub fn on_add_output(
    _trigger: On<AddOutputClicked>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut detail: bevy::prelude::NonSendMut<OutputDetail>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = nodes_of(&refs) else { return };
    let before = doc.raw.clone();
    flush_open(&mut doc, &detail);
    let renames = flush_propagate(&before, &mut doc);
    let key_field = doc
        .raw
        .as_ref()
        .and_then(|r| r.get("scheme"))
        .and_then(|s| s.get("entity"))
        .and_then(|e| e.get("key_field"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let n = {
        let Some(outs) = outputs_mut(&mut doc) else {
            log_err(nodes, &mut godot, "尚未载入方案");
            return;
        };
        let num = outs.len() + 1;
        let mut payload = serde_json::json!({
            "file": "模板.xlsx",
            "apply": "inplace",
            "backup": true,
            "dry_run": true,
            "groups": [],
            "columns": [],
        });
        if !key_field.is_empty() {
            payload["row_key"] = serde_json::Value::String(key_field);
        }
        outs.push(serde_json::json!({
            "id": format!("输出{num}"),
            "kind": { "patch_template": payload },
        }));
        num
    };
    doc.selected_output = n as i32 - 1;
    render_all(nodes, &doc, &mut detail, &mut godot);
    for line in &renames {
        log_plain(nodes, &mut godot, line);
    }
    log_plain(
        nodes,
        &mut godot,
        &format!(
            "已新增输出（共 {n} 个，默认回写模板）：填 id/模板文件，并按需切到“生成新文件”方式"
        ),
    );
}

/// 顶部下拉切换了要编辑的输出（序号 = 控件当前选中项）。
pub fn on_output_picked(
    _trigger: On<OutputPicked>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut detail: bevy::prelude::NonSendMut<OutputDetail>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = nodes_of(&refs) else { return };
    let count = outputs_arr(&doc).len();
    if count == 0 {
        return;
    }
    let idx = godot
        .get::<OptionButton>(nodes.output_pick)
        .get_selected()
        .max(0) as usize;
    if idx >= count {
        return;
    }
    // 切换选中前先把上一个输出详情里的未保存编辑落回 raw，切回时仍在。
    let before = doc.raw.clone();
    flush_open(&mut doc, &detail);
    let renames = flush_propagate(&before, &mut doc);
    doc.selected_output = idx as i32;
    render_all(nodes, &doc, &mut detail, &mut godot);
    for line in &renames {
        log_plain(nodes, &mut godot, line);
    }
    let id = outputs_arr(&doc)
        .get(doc.selected_output as usize)
        .and_then(|o| o.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or("?")
        .to_string();
    log_plain(nodes, &mut godot, &format!("编辑输出：{id}"));
}

/// 「删除当前输出」：删除顶部下拉当前选中的输出。
pub fn on_remove_output(
    _trigger: On<RemoveCurrentOutput>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut detail: bevy::prelude::NonSendMut<OutputDetail>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = nodes_of(&refs) else { return };
    let before = doc.raw.clone();
    flush_open(&mut doc, &detail);
    let renames = flush_propagate(&before, &mut doc);
    let idx = doc.selected_output.max(0) as usize;
    let removed_id = {
        let Some(outs) = outputs_mut(&mut doc) else {
            return;
        };
        if idx >= outs.len() {
            return;
        }
        let rid = outs[idx]
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_string();
        outs.remove(idx);
        rid
    };
    let last = outputs_arr(&doc).len() as i32 - 1;
    if doc.selected_output > last {
        doc.selected_output = last.max(0);
    }
    render_all(nodes, &doc, &mut detail, &mut godot);
    for line in &renames {
        log_plain(nodes, &mut godot, line);
    }
    log_plain(nodes, &mut godot, &format!("已删除输出：{removed_id}"));
}

/// 输出方式下拉被改动：先落库（仍按旧方式面板内容写回），再把该输出的 kind
/// 换成新方式（带默认骨架、保留原 id 与旧 file），随后重建详情。
pub fn on_kind_changed(
    _trigger: On<OutputKindChanged>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut detail: bevy::prelude::NonSendMut<OutputDetail>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = nodes_of(&refs) else { return };
    let Some(kind_btn) = detail.kind_btn.clone() else {
        return;
    };
    let new_kind = kind_key_at(kind_btn.get_selected()).to_string();
    if detail.rendered_kind.as_deref() == Some(new_kind.as_str()) {
        return;
    }
    // 先落库（按旧方式面板内容写回），再从 raw 取回旧 file 用作新方式的默认骨架。
    let before = doc.raw.clone();
    flush_open(&mut doc, &detail);
    let renames = flush_propagate(&before, &mut doc);
    let idx = doc.selected_output.max(0) as usize;
    {
        let Some(outs) = outputs_mut(&mut doc) else {
            return;
        };
        let Some(obj) = outs.get_mut(idx) else {
            return;
        };
        let old_file = obj
            .get("kind")
            .and_then(|k| k.as_object())
            .and_then(|m| m.keys().next().cloned())
            .filter(|k| *k == KIND_PATCH || *k == KIND_NEW_FILE)
            .and_then(|k| obj.get("kind").and_then(|kk| kk.get(&k)))
            .and_then(|p| p.get("file"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let payload = if new_kind == KIND_NEW_FILE {
            serde_json::json!({
                "file": old_file,
                "from": "",
                "sheet": "Sheet1",
                "columns": [],
            })
        } else {
            serde_json::json!({
                "file": old_file,
                "apply": "inplace",
                "backup": true,
                "dry_run": true,
                "groups": [],
                "columns": [],
            })
        };
        obj["kind"] = serde_json::json!({ new_kind.clone(): payload });
    }
    render_all(nodes, &doc, &mut detail, &mut godot);
    for line in &renames {
        log_plain(nodes, &mut godot, line);
    }
    log_warn(
        nodes,
        &mut godot,
        &format!(
            "输出方式已切换为 {}：原方式下的详细配置已重置为默认，请重新填写（输出 id 与文件名已保留）",
            if new_kind == KIND_NEW_FILE {
                "生成新文件"
            } else {
                "回写模板"
            }
        ),
    );
}

/// 新文件“数据来源 from”被改动：落库后清空输出列清单并重建（列候选随新来源刷新）。
pub fn on_from_changed(
    _trigger: On<OutputFromChanged>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut detail: bevy::prelude::NonSendMut<OutputDetail>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = nodes_of(&refs) else { return };
    if detail.rendered_kind.as_deref() != Some(KIND_NEW_FILE) {
        return;
    }
    let new_from = detail
        .nf_from_btn
        .as_ref()
        .map(|b| btn_value(b, &detail.nf_from_opts))
        .unwrap_or_default();
    let before = doc.raw.clone();
    flush_open(&mut doc, &detail);
    let renames = flush_propagate(&before, &mut doc);
    let idx = doc.selected_output.max(0) as usize;
    {
        let Some(outs) = outputs_mut(&mut doc) else {
            return;
        };
        let Some(obj) = outs.get_mut(idx) else {
            return;
        };
        if let Some(nf) = obj
            .get_mut("kind")
            .and_then(|k| k.get_mut(KIND_NEW_FILE))
            .and_then(|p| p.as_object_mut())
        {
            nf.remove("columns");
        }
    }
    render_all(nodes, &doc, &mut detail, &mut godot);
    for line in &renames {
        log_plain(nodes, &mut godot, line);
    }
    log_plain(
        nodes,
        &mut godot,
        &format!(
            "新文件来源表已切换：{}（列候选已刷新，旧列清单已清空）",
            if new_from.is_empty() {
                "（未选）"
            } else {
                &new_from
            }
        ),
    );
}

/// 回写模板“+ 新增 sheet”：在当前输出加一行空的分组→sheet 映射。
pub fn on_add_sheet(
    _trigger: On<AddOutputSheetClicked>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut detail: bevy::prelude::NonSendMut<OutputDetail>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = nodes_of(&refs) else { return };
    let before = doc.raw.clone();
    flush_open(&mut doc, &detail);
    let renames = flush_propagate(&before, &mut doc);
    let idx = doc.selected_output.max(0) as usize;
    let added = {
        let Some(outs) = outputs_mut(&mut doc) else {
            return;
        };
        let Some(obj) = outs.get_mut(idx) else {
            log_err(nodes, &mut godot, "请先选择一个输出");
            return;
        };
        let Some(p) = obj
            .get_mut("kind")
            .and_then(|k| k.get_mut(KIND_PATCH))
            .and_then(|p| p.as_object_mut())
        else {
            log_err(nodes, &mut godot, "当前输出不是回写模板");
            return;
        };
        let rows = p
            .entry("groups")
            .or_insert_with(|| serde_json::Value::Array(Vec::new()));
        let rows = rows.as_array_mut().expect("刚创建");
        rows.push(serde_json::json!({ "group": "", "sheet": "" }));
        rows.len()
    };
    render_all(nodes, &doc, &mut detail, &mut godot);
    for line in &renames {
        log_plain(nodes, &mut godot, line);
    }
    log_plain(
        nodes,
        &mut godot,
        &format!("已加一行分组→sheet 映射（共 {added} 行）：选分组并填模板 sheet 名"),
    );
}

/// 删除当前输出的第 index 个分组→sheet 映射行。
pub fn on_remove_sheet(
    trigger: On<OutputSheetRemoveClicked>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut detail: bevy::prelude::NonSendMut<OutputDetail>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = nodes_of(&refs) else { return };
    let idx = trigger.event().index;
    let before = doc.raw.clone();
    flush_open(&mut doc, &detail);
    let renames = flush_propagate(&before, &mut doc);
    let sel = doc.selected_output.max(0) as usize;
    {
        let Some(outs) = outputs_mut(&mut doc) else {
            return;
        };
        let Some(obj) = outs.get_mut(sel) else {
            return;
        };
        let Some(rows) = obj
            .get_mut("kind")
            .and_then(|k| k.get_mut(KIND_PATCH))
            .and_then(|p| p.get_mut("groups"))
            .and_then(|g| g.as_array_mut())
        else {
            return;
        };
        if idx < 0 || idx as usize >= rows.len() {
            return;
        }
        rows.remove(idx as usize);
    }
    render_all(nodes, &doc, &mut detail, &mut godot);
    for line in &renames {
        log_plain(nodes, &mut godot, line);
    }
    log_plain(
        nodes,
        &mut godot,
        &format!("已删除第 {idx} 个分组→sheet 映射"),
    );
}

/// 回写模板“+ 新增写入列”。
pub fn on_add_col(
    _trigger: On<AddOutputColClicked>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut detail: bevy::prelude::NonSendMut<OutputDetail>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = nodes_of(&refs) else { return };
    let before = doc.raw.clone();
    flush_open(&mut doc, &detail);
    let renames = flush_propagate(&before, &mut doc);
    let sel = doc.selected_output.max(0) as usize;
    let added = {
        let Some(outs) = outputs_mut(&mut doc) else {
            return;
        };
        let Some(obj) = outs.get_mut(sel) else {
            log_err(nodes, &mut godot, "请先选择一个输出");
            return;
        };
        let Some(p) = obj
            .get_mut("kind")
            .and_then(|k| k.get_mut(KIND_PATCH))
            .and_then(|p| p.as_object_mut())
        else {
            log_err(nodes, &mut godot, "当前输出不是回写模板");
            return;
        };
        let rows = p
            .entry("columns")
            .or_insert_with(|| serde_json::Value::Array(Vec::new()));
        let rows = rows.as_array_mut().expect("刚创建");
        rows.push(serde_json::json!({ "field": "", "header": "" }));
        rows.len()
    };
    render_all(nodes, &doc, &mut detail, &mut godot);
    for line in &renames {
        log_plain(nodes, &mut godot, line);
    }
    log_plain(
        nodes,
        &mut godot,
        &format!("已加一个写入列（共 {added} 列）：选字段并填模板表头名"),
    );
}

/// 删除回写模板第 index 个写入列。
pub fn on_remove_col(
    trigger: On<OutputColRemoveClicked>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut detail: bevy::prelude::NonSendMut<OutputDetail>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = nodes_of(&refs) else { return };
    let idx = trigger.event().index;
    let before = doc.raw.clone();
    flush_open(&mut doc, &detail);
    let renames = flush_propagate(&before, &mut doc);
    let sel = doc.selected_output.max(0) as usize;
    {
        let Some(outs) = outputs_mut(&mut doc) else {
            return;
        };
        let Some(obj) = outs.get_mut(sel) else {
            return;
        };
        let Some(rows) = obj
            .get_mut("kind")
            .and_then(|k| k.get_mut(KIND_PATCH))
            .and_then(|p| p.get_mut("columns"))
            .and_then(|c| c.as_array_mut())
        else {
            return;
        };
        if idx < 0 || idx as usize >= rows.len() {
            return;
        }
        let field = rows[idx as usize]
            .get("field")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_string();
        rows.remove(idx as usize);
        log_plain(nodes, &mut godot, &format!("已删除写入列 [{idx}]：{field}"));
    }
    render_all(nodes, &doc, &mut detail, &mut godot);
    for line in &renames {
        log_plain(nodes, &mut godot, line);
    }
}

/// 新文件“+ 新增列”。
pub fn on_add_nf_col(
    _trigger: On<AddOutputNfColClicked>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut detail: bevy::prelude::NonSendMut<OutputDetail>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = nodes_of(&refs) else { return };
    let before = doc.raw.clone();
    flush_open(&mut doc, &detail);
    let renames = flush_propagate(&before, &mut doc);
    let sel = doc.selected_output.max(0) as usize;
    let added = {
        let Some(outs) = outputs_mut(&mut doc) else {
            return;
        };
        let Some(obj) = outs.get_mut(sel) else {
            log_err(nodes, &mut godot, "请先选择一个输出");
            return;
        };
        let Some(p) = obj
            .get_mut("kind")
            .and_then(|k| k.get_mut(KIND_NEW_FILE))
            .and_then(|p| p.as_object_mut())
        else {
            log_err(nodes, &mut godot, "当前输出不是新文件");
            return;
        };
        let rows = p
            .entry("columns")
            .or_insert_with(|| serde_json::Value::Array(Vec::new()));
        let rows = rows.as_array_mut().expect("刚创建");
        rows.push(serde_json::json!({ "header": "", "field": "" }));
        rows.len()
    };
    render_all(nodes, &doc, &mut detail, &mut godot);
    for line in &renames {
        log_plain(nodes, &mut godot, line);
    }
    log_plain(
        nodes,
        &mut godot,
        &format!("已加一个新文件列（共 {added} 列）：填表头名并选来源字段"),
    );
}

/// 删除新文件第 index 个列。
pub fn on_remove_nf_col(
    trigger: On<OutputNfColRemoveClicked>,
    refs: Res<EditorRefs>,
    mut doc: ResMut<EditorDoc>,
    mut detail: bevy::prelude::NonSendMut<OutputDetail>,
    mut godot: GodotAccess,
) {
    let Some(nodes) = nodes_of(&refs) else { return };
    let idx = trigger.event().index;
    let before = doc.raw.clone();
    flush_open(&mut doc, &detail);
    let renames = flush_propagate(&before, &mut doc);
    let sel = doc.selected_output.max(0) as usize;
    {
        let Some(outs) = outputs_mut(&mut doc) else {
            return;
        };
        let Some(obj) = outs.get_mut(sel) else {
            return;
        };
        let Some(rows) = obj
            .get_mut("kind")
            .and_then(|k| k.get_mut(KIND_NEW_FILE))
            .and_then(|p| p.get_mut("columns"))
            .and_then(|c| c.as_array_mut())
        else {
            return;
        };
        if idx < 0 || idx as usize >= rows.len() {
            return;
        }
        let header = rows[idx as usize]
            .get("header")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_string();
        rows.remove(idx as usize);
        log_plain(
            nodes,
            &mut godot,
            &format!("已删除新文件列 [{idx}]：{header}"),
        );
    }
    render_all(nodes, &doc, &mut detail, &mut godot);
    for line in &renames {
        log_plain(nodes, &mut godot, line);
    }
}

// ---------- 写回（保存 / 校验前调用） ----------

/// 当前选中输出的详情（id/kind + 分节内容）整体写回 raw 的输出项，并做基本校验。
/// 分组/字段/来源是否指向现有对象由改名传播与引擎校验统一查，这里只查填写完整性。
pub fn write_back(
    raw: &mut serde_json::Value,
    detail: &OutputDetail,
    selected: i32,
) -> Result<(), String> {
    let Some(outs) = raw
        .get_mut("scheme")
        .and_then(|s| s.get_mut("outputs"))
        .and_then(|v| v.as_array_mut())
    else {
        return Err("缺少 outputs 数组".into());
    };
    let sel = selected as usize;
    if sel >= outs.len() {
        return Ok(()); // 无可写输出
    }
    let id = detail
        .id_edit
        .as_ref()
        .map(|e| e.get_text().to_string())
        .unwrap_or_default();
    let id = id.trim().to_string();
    if id.is_empty() {
        return Err("输出 id 不能为空".into());
    }
    let Some(kind) = detail.rendered_kind.as_deref() else {
        return Ok(());
    };
    // 组装 kind 对象（键 = 输出方式，值 = 该方式的配置对象）。
    let kind_map = if kind == KIND_PATCH {
        gather_patch_payload(detail)
    } else {
        gather_new_file_payload(detail)
    };
    // 校验部分填写/重复行（读 kind_map[kind] 里的配置对象）。
    let inner = kind_map.get(kind).and_then(|v| v.as_object());
    if kind == KIND_PATCH {
        let groups = inner
            .and_then(|p| p.get("groups"))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        for (j, g) in groups.iter().enumerate() {
            let group = g.get("group").and_then(|v| v.as_str()).unwrap_or("");
            let sheet = g.get("sheet").and_then(|v| v.as_str()).unwrap_or("");
            if group.is_empty() && !sheet.is_empty() {
                return Err(format!(
                    "输出[{id}] sheet 映射第 {} 行：选了 sheet 但未选分组",
                    j + 1
                ));
            }
            if !group.is_empty() && sheet.is_empty() {
                return Err(format!(
                    "输出[{id}] sheet 映射第 {} 行：选了分组但未填模板 sheet 名",
                    j + 1
                ));
            }
        }
        if let Some(cols) = inner
            .and_then(|p| p.get("columns"))
            .and_then(|v| v.as_array())
        {
            for (j, c) in cols.iter().enumerate() {
                let field = c.get("field").and_then(|v| v.as_str()).unwrap_or("");
                let header = c.get("header").and_then(|v| v.as_str()).unwrap_or("");
                if field.is_empty() && !header.is_empty() {
                    return Err(format!(
                        "输出[{id}] 写入列第 {} 行：填了模板表头但未选字段",
                        j + 1
                    ));
                }
                if !field.is_empty() && header.is_empty() {
                    return Err(format!(
                        "输出[{id}] 写入列第 {} 行：选了字段但未填模板表头名",
                        j + 1
                    ));
                }
            }
        }
    } else if kind == KIND_NEW_FILE {
        let from = inner
            .and_then(|p| p.get("from"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let sheet = inner
            .and_then(|p| p.get("sheet"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if from.is_empty() {
            return Err(format!("输出[{id}]：请先选数据来源 from"));
        }
        if sheet.is_empty() {
            return Err(format!("输出[{id}]：请填新文件 sheet 名"));
        }
        if let Some(cols) = inner
            .and_then(|p| p.get("columns"))
            .and_then(|v| v.as_array())
        {
            for (j, c) in cols.iter().enumerate() {
                let header = c.get("header").and_then(|v| v.as_str()).unwrap_or("");
                let field = c.get("field").and_then(|v| v.as_str()).unwrap_or("");
                if header.is_empty() || field.is_empty() {
                    return Err(format!(
                        "输出[{id}] 新文件第 {} 行：请同时填表头名并选字段",
                        j + 1
                    ));
                }
            }
        }
    }
    let obj = outs[sel]
        .as_object_mut()
        .ok_or_else(|| format!("输出[{id}] 序列化失败"))?;
    obj.insert("id".into(), serde_json::Value::String(id));
    obj.insert("kind".into(), serde_json::Value::Object(kind_map));
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
