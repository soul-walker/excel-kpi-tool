# 技术说明与选型约定

本文说明本项目使用的技术栈与工程约定。新增功能优先遵守本说明；引入新依赖前，
先在根 `Cargo.toml` 的 `[workspace.dependencies]` 登记，再在成员中以 `workspace = true`
引用（规则见根 `Cargo.toml` 顶部注释）。

## 技术栈总览

| 层 | 技术 |
| --- | --- |
| 编辑器 / 表现层 | Godot 4.x（`godot/` 工程） |
| 游戏逻辑层 | Rust + Bevy ECS，经 godot-bevy 以 GDExtension 嵌入 Godot（`godot/rust`） |
| 服务端 | Bevy headless（`server/`；骨架已生成，网络层待接入，见代码 TODO） |
| 研发工具 | xtask 式 `tools`（`setup` 子命令；后续扩展 build / test / release） |
| 语言 | Rust 稳定版（≥ 1.88，edition 2024），toolchain 由 `rust-toolchain.toml` 锁定 |

## 分层约定

### Godot 只做表现，玩法逻辑一律走 Bevy（godot-bevy）

- 场景、资产、动画、UI 等在 Godot 编辑器中搭建；
- **业务 / 玩法逻辑全部用 Rust + Bevy ECS 实现**（组件、System、Schedule、Event、资源），
  挂在 `#[bevy_app]` 入口（`godot/rust/src/lib.rs`）启动的 App 上；
- GDScript 仅允许用于编辑器插件与薄胶水，**不承载业务逻辑**；
- 与 Godot 的桥接统一使用 godot-bevy 提供的能力（`#[bevy_app]`、TransformSync、
  InputBridge、Audio 等），不绕开它自行注册 GDExtension 入口。

### client / server 边界

- 需要被 client / server / tools 复用的类型（协议消息、公共逻辑）→ 自行新增 `shared` crate
  （建议目录 `shared/`、crate 名 `<crate名>_shared`，先在根 `Cargo.toml` 登记 path）；
- client 专属（Godot 桥接 / 表现）留在 `godot/rust`，server 专属（权威状态 / 网络）留在 `server`。

## 通用选型

| 领域 | 约定 |
| --- | --- |
| 错误处理 | `anyhow`（应用 / 入口层）+ `thiserror`（库内错误类型定义） |
| 序列化 | `serde` + `serde_json`（默认 JSON；二进制协议等性能敏感场景再评估 `bincode`） |
| CLI | `clap`（derive），tools 子命令 |
| 日志 | `tracing` + `tracing-subscriber`（已预置在 workspace.dependencies，按需 `workspace = true` 引入并启用 `fmt` / `env-filter` 等 feature） |
| 异步运行时 | `tokio`（需要 async 时引入；Bevy 侧经 task 插件桥接，避免阻塞帧循环） |

## 特定领域选型

- **关系型数据库**：默认使用 **`sea-orm`**（ORM，配合 `sqlx` 驱动）。数据库访问是异步的，
  优先在 `server` / `tools` 中使用；`client` 内如需本地库，先评估 Godot 主线程与 Bevy
  调度约束再接入。
- **网络（规划）**：headless `server` + 权威状态；协议 / 消息类型放自行新增的 `shared`。
  传输层默认候选 `bevy_replicon`，接入时按依赖规范登记并回写本说明。

## 工程质量

- git 提交钩子（prek）自动执行 `cargo fmt` / `cargo check` / `cargo clippy` / `typos`；
  配置见根目录 `prek.toml` 与 `_typos.toml`；
- 所有 crate 的依赖版本与 features 统一在根 `Cargo.toml` 维护。
