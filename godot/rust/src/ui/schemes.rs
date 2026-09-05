//! 方案库位置：Godot 项目用户数据目录（`user://schemes`）。
//! 首次运行从仓库样例（`res://../schemes`）播种一份，之后用户只在这里增删改。

use godot::classes::ProjectSettings;
use godot::obj::Singleton;

use crate::bridge;

/// 项目用户方案目录（绝对路径）。
pub fn user_schemes_dir() -> String {
    ProjectSettings::singleton()
        .globalize_path("user://schemes")
        .to_string()
}

/// 仓库内置样例方案目录（开发/播种用）。
pub fn sample_schemes_dir() -> String {
    ProjectSettings::singleton()
        .globalize_path("res://../schemes")
        .to_string()
}

/// 确保用户方案目录存在，且首次使用时播种样例。
pub fn ensure_seeded() {
    let _ = bridge::seed_schemes_if_empty(&sample_schemes_dir(), &user_schemes_dir());
}

/// 当前可用的方案名列表（.json，已排序）。
pub fn scheme_names() -> Vec<String> {
    ensure_seeded();
    bridge::list_json_names(&user_schemes_dir())
}

/// 由方案名得到完整路径。
pub fn scheme_path(name: &str) -> String {
    format!("{}/{}", user_schemes_dir(), name)
}
