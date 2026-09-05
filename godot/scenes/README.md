# 场景组织约定

- `main.tscn` 固定在工程最外层：只做“应用壳”（顶栏导航 + 页面内容宿主），不含业务 UI；
- 功能页场景按页面分目录：`scenes/<页面>/<页面>.tscn`（如 `scenes/run/run_console.tscn`、
  `scenes/editor/…`）；
- 跨页面复用的控件/弹层/小部件放 `scenes/common/`；
- 页面切换由 Rust 侧路由完成（顶栏按钮 → typed signal → Bevy observer → 换出/换入 `GodotScene`）；
- 场景内禁止挂 `.gd` 脚本，交互一律经 godot-bevy 类型化信号桥接。
