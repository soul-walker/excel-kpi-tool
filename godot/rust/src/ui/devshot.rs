//! 开发自检：设置 KPI_SHOT=1 时，自动点按钮并把界面截图到 target/ui-shot.png 后退出。
//!
//! 时序（帧）：30 校验 → 60 dry-run → 90 切方案编辑页 →
//! 120 新建方案 → 125 填名称/描述 → 130/140 新增两个参数 →
//! 150 编辑第 1 行（名称=考勤分、值=99.5，自动识别小数）→ 170 第 2 行改名同名 →
//! 185 保存（应报参数名重复）→ 195 第 2 行名恢复 → 210 保存成功 → 225 校验通过 →
//! 232 切数据源页 → 235/240 加源加列 → 248 保存 →
//! 252 切派生页 → 256/262/268 新增派生+映射+字段 → 278 填值 → 288 保存 → 380 截图退出。

use bevy::prelude::{Local, Name, NonSend, Query, Res};
use godot::classes::{Button, Control, LineEdit, ProjectSettings, TabContainer};
use godot::obj::Singleton;
use godot_bevy::prelude::{GodotAccess, GodotNodeHandle};
use tracing::{info, warn};

use super::derived::{DerivedDetail, DerivedListRows};
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

pub fn dev_shot_system(
    mut frame: Local<u32>,
    refs: Res<UiRefs>,
    rows: NonSend<ParamRows>,
    dlist: NonSend<DerivedListRows>,
    ddetail: NonSend<DerivedDetail>,
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
    // 编辑第 1 行：名称=考勤分，输入值 99.5 → 期望自动推断类型为“小数”
    if *frame == 150 {
        if !rows.name_edits.is_empty()
            && !rows.type_buttons.is_empty()
            && !rows.value_edits.is_empty()
        {
            rows.name_edits[0].clone().set_text("考勤分");
            rows.value_edits[0].clone().set_text("99.5");
            // set_text 不触发 text_changed，这里手动发出以模拟真实输入。
            let _ = rows.value_edits[0].clone().emit_signal("text_changed", &[]);
            info!("KPI_SHOT: 输入参数值 99.5（期望自动识别类型为小数）");
        } else {
            warn!("KPI_SHOT: 参数行尚未渲染，跳过行编辑");
        }
    }
    // 删除第 2 行参数（index=1）
    // 把第 2 行也改成同名“考勤分” → 保存应被参数名重复校验拦截
    if *frame == 170 {
        if rows.name_edits.len() >= 2 {
            rows.name_edits[1].clone().set_text("考勤分");
            info!("KPI_SHOT: 第 2 行改名 考勤分（期望保存报重复）");
        } else {
            warn!("KPI_SHOT: 参数行不足，跳过重复名设置");
        }
    }
    // 校验（内存态存在重名 → 应红色报 参数名重复）
    click_button(
        180,
        *frame,
        find("ValidateBtn"),
        "校验(期望红:重名)",
        &mut godot,
    );
    // 保存（应失败：参数名重复）
    click_button(185, *frame, find("SaveBtn"), "保存(期望失败)", &mut godot);
    // 把第 2 行名改回 参数1 → 再次保存应成功
    if *frame == 195 && rows.name_edits.len() >= 2 {
        rows.name_edits[1].clone().set_text("参数1");
        info!("KPI_SHOT: 第 2 行名恢复为 参数1");
    }
    click_button(210, *frame, find("SaveBtn"), "保存", &mut godot);
    // 保存后再校验 → 应输出绿色“校验通过”
    click_button(225, *frame, find("ValidateBtn"), "校验(期望绿)", &mut godot);

    // —— 数据源页签：切到“数据源”，新增数据源与列，再保存 ——
    if *frame == 232
        && let Some(tabs) = find("Tabs")
    {
        godot.get::<TabContainer>(tabs).set_current_tab(1);
        info!("KPI_SHOT: 已切到数据源页签");
    }
    click_button(235, *frame, find("AddSourceBtn"), "新增数据源", &mut godot);
    click_button(240, *frame, find("AddColBtn"), "新增列", &mut godot);
    // 保存（把数据源/列映射写入文件）
    click_button(248, *frame, find("SaveBtn"), "保存(含数据源)", &mut godot);

    // —— 派生页签：新增派生视图并编辑其映射/字段 ——
    if *frame == 252
        && let Some(tabs) = find("Tabs")
    {
        godot.get::<TabContainer>(tabs).set_current_tab(2);
        info!("KPI_SHOT: 已切到派生页签");
    }
    click_button(256, *frame, find("AddDerivedBtn"), "新增派生", &mut godot);
    click_button(262, *frame, find("AddMapBtn"), "新增映射行", &mut godot);
    click_button(268, *frame, find("AddFieldBtn"), "新增字段", &mut godot);
    if *frame == 278 {
        if !dlist.id_edits.is_empty() {
            dlist.id_edits[0].clone().set_text("代课视角");
        }
        if !ddetail.map_dst.is_empty() && !ddetail.map_src.is_empty() {
            ddetail.map_dst[0].clone().set_text("姓名");
            ddetail.map_src[0].clone().set_text("替上课老师");
        }
        if let Some(g) = &ddetail.group_edit {
            g.clone().set_text("姓名");
        }
        if !ddetail.field_name.is_empty() && !ddetail.field_expr.is_empty() {
            ddetail.field_name[0].clone().set_text("节数");
            ddetail.field_expr[0].clone().set_text("SUM(节数)");
        }
        info!("KPI_SHOT: 已填派生（代课视角/姓名←替上课老师/SUM(节数)）");
    }
    click_button(288, *frame, find("SaveBtn"), "保存(含派生)", &mut godot);

    if *frame == 380
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
