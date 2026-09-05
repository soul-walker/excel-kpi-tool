//! 引擎内部的规整表格模型：Reader 解析后统一输出为该类型，
//! 上层计算（聚合/公式）只依赖此模型，不接触 xlsx/XML。

/// 规整表格：列名 + 行数据。
#[derive(Debug, Clone, Default)]
pub struct Table {
    /// 列（顺序与 Row 的 CellValue 对齐）。
    pub columns: Vec<Column>,
    pub rows: Vec<Row>,
}

impl Table {
    pub fn column_index(&self, alias: &str) -> Option<usize> {
        self.columns.iter().position(|c| c.alias == alias)
    }

    pub fn row_count(&self) -> usize {
        self.rows.len()
    }
}

/// 列。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    /// 引擎内引用名（来自方案 ColumnSpec.alias）。
    pub alias: String,
    /// 表头原文（展示/对拍用）。
    pub header: String,
}

/// 一行数据，与 `columns` 对齐。
#[derive(Debug, Clone, Default)]
pub struct Row {
    pub cells: Vec<CellValue>,
}

impl Row {
    pub fn get(&self, idx: usize) -> Option<&CellValue> {
        self.cells.get(idx)
    }
}

/// 单元格值（统一类型，读取时已按列定义转换）。
#[derive(Debug, Clone, PartialEq)]
pub enum CellValue {
    Text(String),
    Integer(i64),
    Float(f64),
    Boolean(bool),
    /// 空单元格。
    Blank,
}

impl CellValue {
    /// 作为数值取用（用于金额/课时等数值列）；文本/空返回 None。
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            CellValue::Integer(i) => Some(*i as f64),
            CellValue::Float(f) => Some(*f),
            CellValue::Boolean(b) => Some(if *b { 1.0 } else { 0.0 }),
            _ => None,
        }
    }

    pub fn is_blank(&self) -> bool {
        matches!(self, CellValue::Blank)
    }
}
