//! 方案（Scheme）v0.1 数据模型。
//!
//! 设计约定（见 docs/设计评审 3.2/3.7）：
//! - 方案是跨月稳定复用的“规则集”，每月只换数据目录（数据源/模板 `file` 均为相对月份目录的文件名）；
//! - 参数作为规则常量内嵌方案，policy 调整时 GUI 编辑并另存新版本；
//! - 内部字段一律英文 snake_case；中文只属于 GUI 显示层；
//! - v0.1 中所有表达式仅以字符串保存，语义求值由 calc DSL（M1）实现；
//! - 存储形态 = 单个 JSON 文件（版本化迁移，见 `VersionedScheme`）。
//!
//! 视图模型：考核以“某人某方面”为点，所有明细基本以方案 `entity.key_field` 的人为主键；
//! 只有“请假替上课”这类一条记录牵涉两个人（请假人 + 替课老师）的台账，才需要派生“另一方视角”
//! 的视图（把行内他人列映射成新 key）。因此 derived 为**单输入**行视图（输入可为 source 或先前
//! derived），靠 `columns` 视图列定义（前缀表达式 + 可选 `GROUP_BY`）表达视角变换，
//! **不需要多表 join/union**（评审确认无此场景）。

use serde::{Deserialize, Serialize};

use crate::calc::{self, Node};
use crate::error::{Error, Result};

/// 当前支持的方案 schema 版本。
pub const SCHEMA_VERSION: u32 = 1;

/// JSON 文件载体：外层带版本号，便于未来迁移。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct VersionedScheme {
    pub schema_version: u32,
    pub scheme: Scheme,
}

impl VersionedScheme {
    /// 从 JSON 文本加载并校验版本。
    pub fn from_json_str(s: &str) -> Result<Self> {
        let parsed: Self = serde_json::from_str(s)?;
        if parsed.schema_version != SCHEMA_VERSION {
            return Err(Error::UnsupportedSchema(
                parsed.schema_version,
                SCHEMA_VERSION,
            ));
        }
        Ok(parsed)
    }

    /// 输出为格式化 JSON（保存用）。
    pub fn to_json_pretty(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }
}

/// 方案主体：一次计算任务的全部规则。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Scheme {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub entity: EntitySpec,
    /// 参数（规则常量，随方案存储与版本化）。
    #[serde(default)]
    pub parameters: Vec<Parameter>,
    /// 输入数据源（台账文件）。
    #[serde(default)]
    pub sources: Vec<SourceSpec>,
    /// 派生明细（过滤/映射/聚合出新表）。
    #[serde(default)]
    pub derived: Vec<DerivedSpec>,
    /// 按 key 的聚合指标。
    #[serde(default)]
    pub aggregates: Vec<AggregateSpec>,
    /// 计算分组（不同公式集）。
    #[serde(default)]
    pub groups: Vec<GroupSpec>,
    /// 输出定义。
    #[serde(default)]
    pub outputs: Vec<OutputSpec>,
}

/// 主维度（人）与 key 规则。
///
/// 去空白/换行/全角空格是**默认** key 归一化（对齐旧代码 HandleStringSpace），无需额外配置。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct EntitySpec {
    /// key 字段名，通常为“姓名”。
    pub key_field: String,
    /// 重名策略。
    #[serde(default)]
    pub duplicate: DuplicatePolicy,
}

/// 重名策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DuplicatePolicy {
    /// 上报问题清单并跳过该 key 的后续写入（推荐，A4/Q2 已确认）。
    #[default]
    Report,
    /// 忽略（后到覆盖先到），仅在用户显式选择时使用。
    Ignore,
}

/// 参数。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Parameter {
    pub name: String,
    #[serde(default)]
    pub label: Option<String>,
    /// 声明的类型（GUI 展示/校验用）。
    #[serde(default)]
    pub value_type: ColumnType,
    pub value: ParamValue,
}

/// 参数的宽松取值（JSON 数值/布尔/字符串）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ParamValue {
    Integer(i64),
    Float(f64),
    Boolean(bool),
    Text(String),
}

/// 数据源。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SourceSpec {
    /// 数据源标识（源内唯一，派生/聚合用 id 引用；可用中文）。
    pub id: String,
    /// 相对月份目录的文件名（如 `课时.xlsx`）；兼容旧写法 `{monthDir}/课时.xlsx`（运行期剥前缀）。
    pub file: String,
    pub sheet: String,
    /// 表头所在行（1 起始）。
    #[serde(default = "default_header_row")]
    pub header_row: u32,
    #[serde(default)]
    pub parse: ParseSpec,
    #[serde(default)]
    pub columns: Vec<ColumnSpec>,
}

fn default_header_row() -> u32 {
    1
}

/// 复杂版式解析技巧。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ParseSpec {
    /// 这些列（按 alias）空值时沿用上一行值（合并单元格/向下填充）。
    #[serde(default)]
    pub fill_down: Vec<String>,
    /// 主从结构：这些列（按 alias）为空时，当前行并入上一条主记录（如“请假替上课”）。
    #[serde(default)]
    pub continue_record_on: Vec<String>,
}

/// 列定义：通过表头名（header）或列号（index）定位。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ColumnSpec {
    #[serde(default)]
    pub header: Option<String>,
    #[serde(default)]
    pub index: Option<u32>,
    /// 引擎内引用名（英文标识符）。
    pub alias: String,
    #[serde(default)]
    pub value_type: ColumnType,
    /// 是否为 key 列。
    #[serde(default)]
    pub key: bool,
    /// 该列数据必须存在（缺失即报错）。
    #[serde(default)]
    pub required: bool,
    /// 数值读取位宽（如 32 表示读到 float32 精度，复刻旧代码 ParseFloat(s,32)）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bits: Option<u32>,
}

/// 列值类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ColumnType {
    #[default]
    Text,
    Integer,
    Decimal,
    Boolean,
    Date,
}

/// 主从续行合并（行视图算子，挂在 DerivedSpec.merge）。
///
/// 同一份物理文件只应声明一次为数据源（sources）；需要“逐物理行”与
/// “续行并入主记录”两种视角时，把后者表达为派生视图：
/// `from` 指向含续行的表，`on` 为“续列”（这些列为空的物理行并入上一条主记录）。
/// 合并规则：数值列求和；文本列以“、”追加（去重）；fill_down/续列本身保持主记录值。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct MergeSpec {
    /// 续列（按 alias）：这些列全为空的物理行并入上一条记录。
    #[serde(default)]
    pub on: Vec<String>,
}

/// 派生视图的一列：`name` = 输出列名；`expr` = 取值表达式。
///
/// 表达式引用一律带命名空间前缀，不允许裸列名：
/// - `{from}.列`：输入表（from）的列，逐行取值（可用在 `SUM(...)` 等聚合里）；
/// - `{视图id}.列`：本视图前面已定义的输出列（引用顺序保证）；
/// - `参数.x`：方案参数；
/// - 分组：某列表达式写 `GROUP_BY({from}.列)`，声明它是组键（全视图至多一列），
///   此时其余列按组上下文求值（可用 `SUM/COUNT/AVG/MIN/MAX(...)`）。
///
/// `expr` 为空（None）＝输出列直接透传输入表同名列（无引用书写）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ColumnDef {
    pub name: String,
    #[serde(default)]
    pub expr: Option<String>,
}

/// 派生明细（表视图）：先做行整理（主从合并），再做行过滤（where），最后按
/// “视图列定义”逐行/分组产出新表。`from` 可引用 source 或先前 derived（视图可链式叠加）。
/// `columns` 为空时表示“原样透传输入行”（配合 merge/where 作行整理视图）。
/// 是否有分组由某列的 `GROUP_BY(...)` 表达式决定，无独立 group_by 字段。
/// 行过滤统一为 `where`（对有无 GROUP_BY 均适用，SQL WHERE 语义）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DerivedSpec {
    pub id: String,
    /// 来源 source / derived 的 id。
    pub from: String,
    /// 可选：主从续行合并（见 MergeSpec）。
    #[serde(default)]
    pub merge: Option<MergeSpec>,
    /// 行过滤表达式（空 = 全部行；引用带 `{from}.` 前缀）。
    #[serde(default, rename = "where")]
    pub where_: Option<String>,
    /// 视图输出列定义（顺序即输出顺序；空 = 原样透传）。
    #[serde(default)]
    pub columns: Vec<ColumnDef>,
}

/// 聚合指标（按 key 对某明细求值）。
///
/// 行过滤在 `where` 字段（SQL WHERE，先裁行再分组/求值）；`expr` 为纯表达式，
/// 行内列带 `{from}.` 前缀（如 `SUM(qingjia.天数)`），引用其它聚合用 `聚合.{id}`。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AggregateSpec {
    pub id: String,
    /// 数据来源（source / derived 的 id）。
    pub from: String,
    /// 按此列分组（from 表的列 alias，通常为 key_field）。
    pub group_by: String,
    /// 聚合前行过滤（空 = 全部行；引用带 `{from}.` 前缀）。
    #[serde(default, rename = "where")]
    pub where_: Option<String>,
    /// 聚合表达式，如 `SUM(qingjia.天数)`。
    pub expr: String,
}

/// 计算分组（一组共享字段 + 覆盖）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct GroupSpec {
    pub id: String,
    pub name: String,
    /// 若按输出模板 sheet 区分分组，这里写 sheet 名。
    #[serde(default)]
    pub match_sheet: Option<String>,
    /// 基础字段（expr 必须存在）。
    #[serde(default)]
    pub fields: Vec<FieldExpr>,
    /// 相对基础字段的覆盖（expr=None 表示停用该字段）。
    #[serde(default)]
    pub overrides: Vec<FieldExpr>,
}

/// 计算字段（或覆盖）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct FieldExpr {
    pub name: String,
    #[serde(default)]
    pub expr: Option<String>,
}

/// 输出。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct OutputSpec {
    pub id: String,
    pub kind: OutputKind,
}

/// 输出方式：回写模板（A/B 可切换，见 3.8）或生成新文件。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputKind {
    /// 打开模板 → 定位行/列 → 只改目标单元格。
    PatchTemplate(PatchTemplate),
    /// 从某明细/计算表生成全新文件（如 补课时.xlsx）。
    NewFile(NewFileOutput),
}

/// 模板回写输出。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PatchTemplate {
    pub file: String,
    /// 回写策略：inplace=写回原文件+自动 .bak；copy=另存结果副本。
    #[serde(default)]
    pub apply: ApplyMode,
    /// 写前备份。
    #[serde(default = "default_true")]
    pub backup: bool,
    /// 运行前 dry-run（先只算不写）。
    #[serde(default = "default_true")]
    pub dry_run: bool,
    /// 模板中定位人员行的 key 列表头名。
    #[serde(default)]
    pub row_key: Option<String>,
    /// 分组 → sheet 的对应。
    #[serde(default)]
    pub groups: Vec<GroupSheetRef>,
    /// 写入列。
    #[serde(default)]
    pub columns: Vec<PatchColumn>,
}

fn default_true() -> bool {
    true
}

/// 回写策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ApplyMode {
    /// 写回原文件（自动 .bak）。
    #[default]
    Inplace,
    /// 另存结果副本。
    Copy,
}

/// 分组 → sheet。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct GroupSheetRef {
    pub group: String,
    pub sheet: String,
}

/// 模板中的写入列。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PatchColumn {
    /// 计算字段名。
    pub field: String,
    /// 模板表头名（按表头定位列）。
    pub header: String,
    /// 数字格式，如 `0`、`0.00`。
    #[serde(default)]
    pub num_fmt: Option<String>,
    /// 空值行为。
    #[serde(default)]
    pub blank: BlankBehavior,
}

/// 空值行为。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BlankBehavior {
    /// 留空单元格。
    #[default]
    Blank,
    /// 写 0。
    Zero,
    /// 保留模板原值不动。
    Keep,
}

/// 生成新文件。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct NewFileOutput {
    pub file: String,
    /// 数据来源（derived id 或 source id）。
    pub from: String,
    pub sheet: String,
    #[serde(default)]
    pub columns: Vec<NewColumn>,
}

/// 新文件列。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct NewColumn {
    pub header: String,
    /// 来源字段（列 alias 或计算字段名）。
    pub field: String,
}

/// 校验问题级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

/// 校验 / 运行期问题（带定位信息，供 GUI 展示与问题清单）。
#[derive(Debug, Clone)]
pub struct Issue {
    pub severity: Severity,
    pub location: String,
    pub message: String,
}

impl Issue {
    fn error(location: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            location: location.into(),
            message: message.into(),
        }
    }
}

impl Scheme {
    /// 静态校验（编译期）。不读文件、不求值；返回全部问题而非 panic。
    pub fn validate(&self) -> Vec<Issue> {
        let mut out = Vec::new();

        // —— id 唯一性（各自命名空间）——
        check_unique(
            &mut out,
            "parameters",
            self.parameters.iter().map(|p| p.name.as_str()),
        );
        check_unique(
            &mut out,
            "sources",
            self.sources.iter().map(|s| s.id.as_str()),
        );
        check_unique(
            &mut out,
            "derived",
            self.derived.iter().map(|d| d.id.as_str()),
        );
        check_unique(
            &mut out,
            "aggregates",
            self.aggregates.iter().map(|a| a.id.as_str()),
        );
        check_unique(
            &mut out,
            "groups",
            self.groups.iter().map(|g| g.id.as_str()),
        );
        check_unique(
            &mut out,
            "outputs",
            self.outputs.iter().map(|o| o.id.as_str()),
        );

        // —— 数据源：同文件+同 sheet+同解析 只声明一次；不同解析视角建议用 derived 视图 ——
        // 兼容旧方案（continue_record_on 源级解析仍可用），因此仅拦截完全相同的重复声明。
        let mut seen_filesheet: Vec<(String, String, String)> = Vec::new();
        for src in &self.sources {
            let key = (
                src.file.clone(),
                src.sheet.clone(),
                src.parse.continue_record_on.join(","),
            );
            if let Some((f, s, _)) = seen_filesheet
                .iter()
                .find(|k| k.0 == key.0 && k.1 == key.1 && k.2 == key.2)
            {
                out.push(Issue::error(
                    format!("sources.{}", src.id),
                    format!("与数据源 {f} 重复声明（同一文件 {s} 且解析相同）；不同视角请用 derived 视图"),
                ));
            }
            seen_filesheet.push(key);
        }

        // —— 数据源列 ——
        for src in &self.sources {
            for col in &src.columns {
                if col.header.is_none() && col.index.is_none() {
                    out.push(Issue::error(
                        format!("sources.{}.columns.{}", src.id, col.alias),
                        "列未指定定位方式（header 与 index 至少其一）",
                    ));
                }
            }
            check_unique(
                &mut out,
                &format!("sources.{} 的列 alias", src.id),
                src.columns.iter().map(|c| c.alias.as_str()),
            );
            let has_key = src.columns.iter().any(|c| c.key);
            if !has_key {
                out.push(Issue::error(
                    format!("sources.{}", src.id),
                    "数据源缺少 key 列（没有列标记 key=true）",
                ));
            }
            for fd in &src.parse.fill_down {
                if !src.columns.iter().any(|c| &c.alias == fd) {
                    out.push(Issue::error(
                        format!("sources.{}.parse.fill_down", src.id),
                        format!("fill_down 引用了不存在的列 alias: {fd}"),
                    ));
                }
            }
            for cr in &src.parse.continue_record_on {
                if !src.columns.iter().any(|c| &c.alias == cr) {
                    out.push(Issue::error(
                        format!("sources.{}.parse.continue_record_on", src.id),
                        format!("continue_record_on 引用了不存在的列 alias: {cr}"),
                    ));
                }
            }
        }

        // —— 派生/聚合 的 from 引用与表达式前缀 ——
        let refs_of = |id: &str| {
            self.sources.iter().any(|s| s.id == id)
                || self.derived.iter().any(|d| d.id == id)
                || self.aggregates.iter().any(|a| a.id == id)
        };
        for d in &self.derived {
            if !refs_of(&d.from) {
                out.push(Issue::error(
                    format!("derived.{}", d.id),
                    format!("from 引用了不存在的 id: {}", d.from),
                ));
            }
            // 视图列名唯一
            check_unique(
                &mut out,
                &format!("derived.{} 的输出列", d.id),
                d.columns.iter().map(|c| c.name.as_str()),
            );
            // 分组：GROUP_BY(...) 作为某列表达式声明（至多一列）
            let group_cols = d
                .columns
                .iter()
                .filter(|c| {
                    c.expr
                        .as_deref()
                        .and_then(|t| calc::parse(t).ok())
                        .is_some_and(|n| is_group_key(&n))
                })
                .count();
            if group_cols > 1 {
                out.push(Issue::error(
                    format!("derived.{}", d.id),
                    "GROUP_BY(...) 只能出现在一列（分组键）",
                ));
            }
            // 列表达式语法与命名空间（引用一律带前缀：from / 本视图 / 参数）
            validate_view_expr(&mut out, d, &format!("derived.{} where", d.id), &d.where_);
            for c in &d.columns {
                validate_view_expr(
                    &mut out,
                    d,
                    &format!("derived.{}.columns.{}", d.id, c.name),
                    &c.expr,
                );
            }
        }
        for a in &self.aggregates {
            if !refs_of(&a.from) {
                out.push(Issue::error(
                    format!("aggregates.{}", a.id),
                    format!("from 引用了不存在的 id: {}", a.from),
                ));
            }
            // 聚合的 where / expr：引用带 {from} / 聚合 / 参数 前缀。
            validate_agg_expr(
                &mut out,
                a,
                &format!("aggregates.{} where", a.id),
                &a.where_,
            );
            validate_agg_expr(
                &mut out,
                a,
                &format!("aggregates.{} expr", a.id),
                &Some(a.expr.clone()),
            );
        }

        // —— 聚合指标之间的引用：`聚合.x` 必须指向已声明指标，且不得成环 ——
        // （前向引用合法：执行器会按依赖拓扑排序计算，见 exec::aggregate_compute_order）
        let agg_ids: Vec<&str> = self.aggregates.iter().map(|a| a.id.as_str()).collect();
        let mut agg_dep: Vec<Vec<usize>> = vec![Vec::new(); self.aggregates.len()];
        for (i, a) in self.aggregates.iter().enumerate() {
            for text in [Some(a.expr.as_str()), a.where_.as_deref()] {
                let Some(text) = text else { continue };
                let Ok(node) = calc::parse(text) else {
                    continue; // 解析失败已由上方 validate_agg_expr 报告
                };
                let mut paths = Vec::new();
                collect_paths(&node, &mut paths);
                for p in paths {
                    if p.len() < 2 || p[0] != "聚合" {
                        continue;
                    }
                    match agg_ids.iter().position(|id| *id == p[1].as_str()) {
                        Some(j) => {
                            if !agg_dep[i].contains(&j) {
                                agg_dep[i].push(j);
                            }
                        }
                        None => out.push(Issue::error(
                            format!("aggregates.{}", a.id),
                            format!("引用了不存在的聚合指标: 聚合.{}", p[1]),
                        )),
                    }
                }
            }
        }
        // 环检测：重复剥除“依赖已全部就绪”的节点，剥不动即为环。
        let mut done = vec![false; self.aggregates.len()];
        loop {
            let mut progressed = false;
            for i in 0..self.aggregates.len() {
                if !done[i] && agg_dep[i].iter().all(|&j| done[j]) {
                    done[i] = true;
                    progressed = true;
                }
            }
            if !progressed {
                break;
            }
        }
        let stuck: Vec<&str> = agg_ids
            .iter()
            .enumerate()
            .filter(|(i, _)| !done[*i])
            .map(|(_, id)| *id)
            .collect();
        if !stuck.is_empty() {
            out.push(Issue::error(
                "aggregates".to_string(),
                format!(
                    "聚合指标存在循环引用（无法确定计算顺序）: {}",
                    stuck.join(" → ")
                ),
            ));
        }

        // —— 分组字段/覆盖 ——
        for g in &self.groups {
            for f in &g.fields {
                if f.expr.is_none() {
                    out.push(Issue::error(
                        format!("groups.{}.fields.{}", g.id, f.name),
                        "基础字段的表达式不能为空",
                    ));
                }
            }
            check_unique(
                &mut out,
                &format!("groups.{} 的字段名", g.id),
                g.fields.iter().map(|f| f.name.as_str()),
            );
            // 基础字段/覆盖表达式语法：与运行期一致先 parse——旧 IF(c,a,b) 等写法
            // 在这里就报错，避免“校验通过但运行失败”。
            for (rows, kind) in [(&g.fields, "fields"), (&g.overrides, "overrides")] {
                for f in rows {
                    let Some(text) = f.expr.as_deref() else {
                        continue;
                    };
                    if text.trim().is_empty() {
                        continue;
                    }
                    if let Err(e) = calc::parse(text) {
                        out.push(Issue::error(
                            format!("groups.{}.{}.{}", g.id, kind, f.name),
                            format!("表达式解析失败: {e}"),
                        ));
                    }
                }
            }
        }

        // —— 输出 ——
        for o in &self.outputs {
            match &o.kind {
                OutputKind::PatchTemplate(p) => {
                    for grp in &p.groups {
                        if !self.groups.iter().any(|g| g.id == grp.group) {
                            out.push(Issue::error(
                                format!("outputs.{}.groups", o.id),
                                format!("引用了不存在的分组: {}", grp.group),
                            ));
                        }
                    }
                }
                OutputKind::NewFile(n) => {
                    if !refs_of(&n.from) {
                        out.push(Issue::error(
                            format!("outputs.{}.from", o.id),
                            format!("from 引用了不存在的 id: {}", n.from),
                        ));
                    }
                }
            }
        }

        out
    }

    /// 校验并返回 `Err`（GUI/CLI 顶层用）；问题合并进 message。
    pub fn ensure_valid(&self) -> Result<()> {
        let issues = self.validate();
        let errors: Vec<_> = issues
            .iter()
            .filter(|i| i.severity == Severity::Error)
            .map(|i| format!("{}: {}", i.location, i.message))
            .collect();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(Error::InvalidScheme(errors.join("\n")))
        }
    }
}

fn check_unique<'a>(out: &mut Vec<Issue>, ns: &str, iter: impl Iterator<Item = &'a str>) {
    let mut seen = std::collections::HashSet::new();
    for name in iter {
        if !seen.insert(name.to_string()) {
            out.push(Issue::error(
                ns.to_string(),
                format!("存在重复 id/name: {name}"),
            ));
        }
    }
}

/// 表达式是否为“分组键”声明（顶层 GROUP_BY(...)）。
fn is_group_key(node: &Node) -> bool {
    matches!(
        node,
        Node::Call { name, .. } if name.eq_ignore_ascii_case("group_by")
    )
}

/// 视图表达式（where / 列 expr）语法与命名空间校验：引用必须带前缀，
/// 且前缀只能是 {from}（输入行）、{本视图 id}（前列）、`参数`。
fn validate_view_expr(out: &mut Vec<Issue>, d: &DerivedSpec, loc: &str, expr: &Option<String>) {
    let Some(text) = expr.as_deref() else { return };
    let node = match calc::parse(text) {
        Ok(n) => n,
        Err(e) => {
            out.push(Issue::error(
                loc.to_string(),
                format!("表达式解析失败: {e}"),
            ));
            return;
        }
    };
    let mut paths = Vec::new();
    collect_paths(&node, &mut paths);
    let allowed: Vec<&str> = vec![d.from.as_str(), d.id.as_str(), "参数"];
    for p in paths {
        if p.len() == 1 {
            out.push(Issue::error(
                loc.to_string(),
                format!(
                    "引用需带命名空间前缀（如 {}.列 / {}.列 / 参数.x）：{}",
                    d.from,
                    d.id,
                    p.join(".")
                ),
            ));
        } else if !allowed.contains(&p[0].as_str()) {
            out.push(Issue::error(
                loc.to_string(),
                format!(
                    "未知命名空间前缀 {}：应为 {} / {} / 参数",
                    p[0], d.from, d.id
                ),
            ));
        }
    }
}

fn collect_paths(node: &Node, out: &mut Vec<Vec<String>>) {
    match node {
        Node::Num(_) | Node::Str(_) | Node::Bool(_) | Node::Empty => {}
        Node::Path(p) => out.push(p.clone()),
        Node::Not(x) => collect_paths(x, out),
        Node::Coalesce { l, r } => {
            collect_paths(l, out);
            collect_paths(r, out);
        }
        Node::If { cond, then, els } => {
            collect_paths(cond, out);
            collect_paths(then, out);
            collect_paths(els, out);
        }
        Node::Bin { l, r, .. } => {
            collect_paths(l, out);
            collect_paths(r, out);
        }
        Node::Call { args, .. } => {
            for a in args {
                collect_paths(a, out);
            }
        }
        Node::Agg { expr, .. } => collect_paths(expr, out),
    }
}

/// 聚合的 where / expr 引用前缀校验：允许 `{from}.列`（行内）、`聚合.x`（已声明的其它指标；
/// 执行时按依赖自动拓扑排序，不要求“前序”）、`参数.x`。
fn validate_agg_expr(out: &mut Vec<Issue>, a: &AggregateSpec, loc: &str, expr: &Option<String>) {
    let Some(text) = expr.as_deref() else { return };
    let node = match calc::parse(text) {
        Ok(n) => n,
        Err(e) => {
            out.push(Issue::error(
                loc.to_string(),
                format!("表达式解析失败: {e}"),
            ));
            return;
        }
    };
    let mut paths = Vec::new();
    collect_paths(&node, &mut paths);
    let allowed: Vec<&str> = vec![a.from.as_str(), "聚合", "参数"];
    for p in paths {
        if p.len() == 1 {
            out.push(Issue::error(
                loc.to_string(),
                format!(
                    "引用需带命名空间前缀（如 {}.列 / 聚合.x / 参数.x）：{}",
                    a.from,
                    p.join(".")
                ),
            ));
        } else if !allowed.contains(&p[0].as_str()) {
            out.push(Issue::error(
                loc.to_string(),
                format!("未知命名空间前缀 {}：应为 {} / 聚合 / 参数", p[0], a.from),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 最小合法方案（仅用于单测，验证加载 + 静态校验链路）。
    const MINIMAL_JSON: &str = r#"{
        "schema_version": 1,
        "scheme": {
            "name": "单测最小方案",
            "entity": { "key_field": "姓名", "duplicate": "report" },
            "parameters": [
                { "name": "课时费", "label": "课时费", "value_type": "decimal", "value": 1.5 }
            ],
            "sources": [
                {
                    "id": "keshi",
                    "file": "{monthDir}/课时.xlsx",
                    "sheet": "课时",
                    "columns": [
                        { "header": "姓名", "alias": "姓名", "value_type": "text", "key": true },
                        { "header": "总计", "alias": "课时", "value_type": "decimal", "required": true }
                    ]
                }
            ],
            "groups": [
                {
                    "id": "fxp",
                    "name": "非校聘",
                    "fields": [
                        { "name": "课时S", "expr": "if false { 0 } else { keshi.课时 ?? 0 }" },
                        { "name": "课时费S", "expr": "课时S * 参数.课时费" }
                    ]
                }
            ],
            "outputs": [
                {
                    "id": "jx",
                    "kind": {
                        "patch_template": {
                            "file": "{monthDir}/绩效.xlsx",
                            "row_key": "姓名",
                            "groups": [ { "group": "fxp", "sheet": "非校聘" } ],
                            "columns": [
                                { "field": "课时S", "header": "课时S", "num_fmt": "0.00" },
                                { "field": "课时费S", "header": "课时费S", "num_fmt": "0.00" }
                            ]
                        }
                    }
                }
            ]
        }
    }"#;

    #[test]
    fn loads_and_validates_minimal() {
        let v = VersionedScheme::from_json_str(MINIMAL_JSON).expect("JSON 应可解析");
        assert_eq!(v.schema_version, SCHEMA_VERSION);
        let issues = v.scheme.validate();
        assert!(
            issues.iter().all(|i| i.severity != Severity::Error),
            "最小方案不应有 error 级问题: {issues:?}"
        );
        v.scheme.ensure_valid().expect("ensure_valid 应通过");
    }

    /// 绩效复刻方案（M1 全量字段链 + 特殊函数引用）应始终可解析且无 error。
    #[test]
    fn loads_replication_scheme_from_repo() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../schemes/高中月绩效复刻.json"
        );
        let raw = std::fs::read_to_string(path).expect("schemes/高中月绩效复刻.json 应存在");
        let v = VersionedScheme::from_json_str(&raw).expect("复刻方案应可解析");
        let issues = v.scheme.validate();
        let errors: Vec<_> = issues
            .iter()
            .filter(|i| i.severity == Severity::Error)
            .map(|i| format!("{}: {}", i.location, i.message))
            .collect();
        assert!(errors.is_empty(), "复刻方案不应有 error: {errors:#?}");
    }

    #[test]
    fn loads_sample_scheme_from_repo() {
        // schemes/ 下的样例方案应始终可解析且无 error（仓库内回归）。
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../schemes/高中月绩效.scheme.json"
        );
        let raw = std::fs::read_to_string(path).expect("schemes/高中月绩效.scheme.json 应存在");
        let v = VersionedScheme::from_json_str(&raw).expect("样例方案应可解析");
        let issues = v.scheme.validate();
        let errors: Vec<_> = issues
            .iter()
            .filter(|i| i.severity == Severity::Error)
            .map(|i| format!("{}: {}", i.location, i.message))
            .collect();
        assert!(errors.is_empty(), "样例方案不应有 error: {errors:#?}");
    }

    /// 计算分组字段里残留旧 IF(c,a,b) 写法：静态校验应直接报“已移除”提示，
    /// 而不是等运行时（dry-run/写回）才失败。
    #[test]
    fn legacy_if_in_group_field_rejected_at_validate() {
        let raw = r#"{
            "schema_version": 1,
            "scheme": {
                "name": "旧写法回归",
                "entity": { "key_field": "姓名", "duplicate": "report" },
                "groups": [
                    {
                        "id": "fxp",
                        "name": "非校聘",
                        "fields": [
                            { "name": "课时S", "expr": "IF(false, 0, 1)" }
                        ]
                    }
                ]
            }
        }"#;
        let v = VersionedScheme::from_json_str(raw).expect("JSON 应可解析");
        let issues = v.scheme.validate();
        assert!(
            issues
                .iter()
                .any(|i| { i.severity == Severity::Error && i.message.contains("已移除") }),
            "校验应报旧 IF 函数写法已被移除: {issues:?}"
        );
    }

    /// 聚合指标之间的引用校验：前向引用合法、未定义引用报错、循环引用报错。
    #[test]
    fn aggregate_cross_references_validated() {
        let source = r#"
            "sources": [
                {
                    "id": "qingjia",
                    "file": "请假替上课.xlsx",
                    "sheet": "请假替上课",
                    "columns": [
                        { "header": "姓名", "alias": "姓名", "value_type": "text", "key": true },
                        { "header": "天数", "alias": "天数", "value_type": "decimal" }
                    ]
                }
            ]"#;
        let scheme_json = |aggregates: &str| {
            format!(
                r#"{{
                "schema_version": 1,
                "scheme": {{
                    "name": "聚合引用校验",
                    "entity": {{ "key_field": "姓名", "duplicate": "report" }},
                    {source},
                    "aggregates": {aggregates}
                }}
            }}"#
            )
        };

        // 前向引用合法（执行器按依赖自动排序），不应新增 error。
        let fwd = scheme_json(
            r#"[
                { "id": "甲", "from": "qingjia", "group_by": "姓名", "expr": "聚合.乙 + 1" },
                { "id": "乙", "from": "qingjia", "group_by": "姓名", "expr": "SUM(qingjia.天数)" }
            ]"#,
        );
        let v = VersionedScheme::from_json_str(&fwd).expect("fwd 应可解析");
        let issues = v.scheme.validate();
        assert!(
            !issues.iter().any(|i| {
                i.severity == Severity::Error
                    && (i.message.contains("循环引用") || i.message.contains("不存在的聚合指标"))
            }),
            "前向引用不应报错: {issues:?}"
        );

        // 未定义引用 → 报错。
        let undef = scheme_json(
            r#"[
                { "id": "甲", "from": "qingjia", "group_by": "姓名", "expr": "聚合.不存在 + 1" }
            ]"#,
        );
        let v = VersionedScheme::from_json_str(&undef).expect("undef 应可解析");
        let issues = v.scheme.validate();
        assert!(
            issues.iter().any(|i| {
                i.severity == Severity::Error && i.message.contains("不存在的聚合指标")
            }),
            "未定义引用应报错: {issues:?}"
        );

        // 循环引用 → 报错。
        let cycle = scheme_json(
            r#"[
                { "id": "甲", "from": "qingjia", "group_by": "姓名", "expr": "聚合.乙 + 1" },
                { "id": "乙", "from": "qingjia", "group_by": "姓名", "expr": "聚合.甲 + 1" }
            ]"#,
        );
        let v = VersionedScheme::from_json_str(&cycle).expect("cycle 应可解析");
        let issues = v.scheme.validate();
        assert!(
            issues
                .iter()
                .any(|i| i.severity == Severity::Error && i.message.contains("循环引用")),
            "循环引用应报错: {issues:?}"
        );
    }
}
