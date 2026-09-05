//! 开发自检：设置 KPI_SHOT=1 时，自动点按钮并把界面截图到 target/ui-shot.png 后退出。
//!
//! 时序（帧）：30 校验 → 60 dry-run → 90 切方案编辑页 →
//! 120 新建方案 → 125 填名称/描述 → 130/140 新增两个参数 →
//! 150 编辑第 1 行（名称=考勤分、类型=小数、值=99.5）→ 180 保存（改名落盘）
//! → 260 截图退出。

use bevy::prelude::{Local, Name, NonSend, Query, Res};
use godot::classes::{Button, Control, LineEdit, ProjectSettings};
use godot::obj::Singleton;
use godot_bevy::prelude::{GodotAccess, GodotNodeHandle};
use tracing::{info, warn};

use super::editor::ParamRows;
use super::view::UiRefs;

fn click_button(
    at_frame: u32,
    now: u32,
    handle: Option<GodotNodeHandle>,
    name: &str,
    godot: &mut GodotAccess,
) {
    if now == at_frame
        && let Some(h) = handle
    {
        info!("KPI_SHOT: 自动点击 {name}");
        let _ = godot.get::<Button>(h).emit_signal("pressed", &[]);
    }
}

/// 点击 ParamRows 里存储的动态删除按钮（Gd 非 Send，只能存 NonSend）。
fn click_remove_button(at_frame: u32, now: u32, rows: &ParamRows, index: usize) {
    if now == at_frame
        && let Some((_, button)) = rows.remove_buttons.get(index)
    {
        info!("KPI_SHOT: 自动点击 删除参数[{index}]");
        let mut b = button.clone();
        let _ = b.emit_signal("pressed", &[]);
    }
}

pub fn dev_shot_system(
    mut frame: Local<u32>,
    refs: Res<UiRefs>,
    rows: NonSend<ParamRows>,
    scene_nodes: Query<(&GodotNodeHandle, &Name)>,
    mut godot: GodotAccess,
) {
    if std::env::var("KPI_SHOT").ok().as_deref() != Some("1") {
        return;
    }
    *frame += 1;

    let find = |name: &str| {
        scene_nodes
            .iter()
            .find(|(_, n)| n.as_str() == name)
            .map(|(h, _)| *h)
    };
    let nodes = refs.nodes.as_ref();
    click_button(
        30,
        *frame,
        nodes.map(|n| n.validate_btn),
        "校验",
        &mut godot,
    );
    click_button(60, *frame, nodes.map(|n| n.dry_btn), "Dry-run", &mut godot);
    // 切到方案编辑器页（自动载入默认方案、渲染参数行）
    click_button(90, *frame, find("EditorNavBtn"), "切到方案编辑", &mut godot);
    // 新建方案（内存草稿，预填“未命名方案-<ts>”）
    click_button(120, *frame, find("NewBtn"), "新建方案", &mut godot);
    // 填名称/描述（随后保存将按“自检方案”改名落盘）
    if *frame == 125 {
        if let (Some(name), Some(desc)) = (find("NameEdit"), find("DescEdit")) {
            godot.get::<LineEdit>(name).set_text("自检方案");
            godot
                .get::<LineEdit>(desc)
                .set_text("KPI_SHOT 自检生成，可删除");
            info!("KPI_SHOT: 已填名称/描述");
        } else {
            warn!("KPI_SHOT: 未找到 NameEdit/DescEdit");
        }
    }
    // 新增两个参数
    click_button(130, *frame, find("AddParamBtn"), "新增参数[0]", &mut godot);
    click_button(140, *frame, find("AddParamBtn"), "新增参数[1]", &mut godot);
    // 编辑第 1 行：名称=考勤分 / 类型=小数 / 值=99.5
    if *frame == 150 {
        if !rows.name_edits.is_empty()
            && !rows.type_buttons.is_empty()
            && !rows.value_edits.is_empty()
        {
            rows.name_edits[0].clone().set_text("考勤分");
            rows.type_buttons[0].clone().select(2); // PARAM_TYPES 下标 2 = decimal
            rows.value_edits[0].clone().set_text("99.5");
            info!("KPI_SHOT: 已编辑第 1 行参数（考勤分/小数/99.5）");
        } else {
            warn!("KPI_SHOT: 参数行尚未渲染，跳过行编辑");
        }
    }
    // 删除第 2 行参数（index=1）
    click_remove_button(160, *frame, &rows, 1);
    // 保存（改名 → 写 schemes/自检方案.json）
    click_button(180, *frame, find("SaveBtn"), "保存", &mut godot);

    if *frame == 260
        && let Some(handle) = find("AppTitle")
    {
        info!("KPI_SHOT: 截图并退出");
        let control = godot.get::<Control>(handle);
        let shot = control
            .get_viewport()
            .and_then(|vp| vp.get_texture())
            .and_then(|tex| tex.get_image());
        if let Some(img) = shot {
            let out = ProjectSettings::singleton()
                .globalize_path("res://../target/ui-shot.png")
                .to_string();
            let err = img.save_png(&out);
            info!("KPI_SHOT saved={out} err={err:?}");
        } else {
            warn!("KPI_SHOT: 未能读取视口图像");
        }
        if let Some(mut tree) = control.get_tree() {
            tree.quit();
        }
    }
}
