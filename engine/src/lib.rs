//! kpi-engine：通用 Excel 薪酬/绩效计算引擎（headless，可被 Godot / CLI / 测试复用）。
//!
//! 仓库结构见根 README 与 `docs/通用Excel薪酬工具-架构设计评审.html` 3.5。
//! M0 范围：crate 骨架 + 方案(scheme) v0.1 模型/静态校验 + 表格/工作簿抽象 + Bevy Plugin 挂载点。
//! M1 范围：reader / calc DSL / DAG 执行 / writer（umya）→ 对拍 school-kpi 输出。

pub mod error;
pub mod plugin;
pub mod scheme;
pub mod table;
pub mod workbook;

pub use error::{Error, Result};
pub use scheme::{
    AggregateSpec, ApplyMode, BlankBehavior, ColumnSpec, ColumnType, DerivedSpec, DuplicatePolicy,
    EntitySpec, FieldExpr, GroupSheetRef, GroupSpec, Issue, NewColumn, NewFileOutput, OutputKind,
    OutputSpec, ParamValue, Parameter, ParseSpec, PatchColumn, PatchTemplate, SCHEMA_VERSION,
    Scheme, Severity, SourceSpec, VersionedScheme,
};
pub use table::{CellValue, Column, Row, Table};
pub use workbook::{UmyaWorkbook, WorkbookIO};
