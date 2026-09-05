//! 开发自检：设置 KPI_SHOT=1 时，自动点按钮并把界面截图到 target/ui-shot.png 后退出。
//!
//! 时序（帧）：30 校验 → 60 dry-run → 90 切方案编辑页 →
//! 140 新建方案 → 190/240 各新增一个参数 → 290 删除第 2 个参数 →
//! 340 保存 → 400 截图退出。

use bevy::prelude::{Local, Name, NonSend, Query, Res};
use godot::classes::{Button, Control, ProjectSettings};
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
    // 编辑器：新建方案（自动载入空参数的新方案）
    click_button(140, *frame, find("NewBtn"), "新建方案", &mut godot);
    // 编辑器：连续新增两个参数
    click_button(190, *frame, find("AddParamBtn"), "新增参数[0]", &mut godot);
    click_button(240, *frame, find("AddParamBtn"), "新增参数[1]", &mut godot);
    // 编辑器：删除第 2 个参数（index=1）
    click_remove_button(290, *frame, &rows, 1);
    // 编辑器：保存（写回方案文件）
    click_button(340, *frame, find("SaveBtn"), "保存", &mut godot);

    if *frame == 400
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
