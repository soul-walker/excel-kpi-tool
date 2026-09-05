# kpi_tool

Godot + Rust（godot-bevy / Bevy ECS）游戏项目（纯客户端）。

- Godot 工程：`godot/`（内含 `addons/godot-bevy` 编辑器插件，Rust 逻辑在 `godot/rust/`）
- Rust workspace：`godot/rust`（client）+ `tools`（研发工具）
- 依赖统一在根 `Cargo.toml` 的 `[workspace.dependencies]` 管理，成员 crate 一律 `workspace = true` 继承
- 需要组件 / 协议等跨 crate 共享代码时，按规范自行新增 `shared` crate（命名与登记见根 `Cargo.toml` 注释）
- 技术栈与选型约定（godot-bevy / Bevy ECS、sea-orm 等）见 [`docs/tech.md`](docs/tech.md)

## 环境安装与配置

### 必需工具

| 工具 | 用途 | 安装方式 |
| --- | --- | --- |
| Rust（稳定版 ≥ 1.88） | 构建 client / server / tools | https://rustup.rs |
| git | 版本管理 | https://git-scm.com |
| prek | git 提交钩子框架（pre-commit 的 Rust 实现） | `cargo install prek` |
| typos-cli | 拼写检查（供 prek 钩子调用） | `cargo install typos-cli` |
| Godot 4.x | 运行 / 编辑 `godot/` 工程 | https://godotengine.org |

### 一键配置开发环境

在项目根执行：

```sh
cargo run -p tools -- setup
```

该命令会：

1. 检查 `prek` / `typos-cli` 是否已安装（缺失时提示安装命令）；
2. 若目录还不是 git 仓库，执行 `git init -b main`；
3. 执行 `prek install` 安装提交钩子（写入 `.git/hooks/pre-commit`）。

提交时自动执行：`cargo fmt -- --check`、`cargo check --all`、`cargo clippy -D warnings`、
`typos` 拼写检查。钩子与拼写配置见根目录 `prek.toml` 与 `_typos.toml`。

> `end-of-file-fixer` / `trailing-whitespace` 等钩子会自动修改文件，首次提交若提示文件被改动，
> `git add` 后再提交即可。

## 运行

```sh
cargo build            # workspace 根构建（gdextension 指向 res://../target）
```

然后用 Godot 打开 `godot/` 工程并 F5 运行。

> 先 `cargo build` 再打开 Godot：autoload 场景 `bevy_app_singleton.tscn` 引用的 `BevyApp`
> 类由 Rust 侧（`godot/rust`）注册。
