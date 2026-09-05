//! xlsx 读写适配层：`WorkbookIO` trait 隔离第三方库（umya-spreadsheet）。
//!
//! 本层是 M0 Spike 的主战场——验证“打开模板 → 只改目标单元格 → 保存后
//! 样式/合并/筛选/公式缓存不被破坏”（含 WPS 产物）。若 umya 兼容性不足，
//! 在此 trait 后补一个 “worksheet XML 最小 patch” 实现，不牵动上层。

use std::path::Path;

use crate::error::{Error, Result};
use crate::scheme::SourceSpec;
use crate::table::Table;

/// 工作簿读写接口。
pub trait WorkbookIO {
    /// 打开模板文件（保留原格式的修改入口）。
    fn open_template(&mut self, path: &Path) -> Result<()>;

    /// 将某数据源（含列映射）读取为规整表格。
    fn read_source(&mut self, src: &SourceSpec, month_dir: &Path) -> Result<Table>;

    /// 写一个单元格（行/列均为 1 起始）。
    fn write_cell(
        &mut self,
        sheet: &str,
        row: u32,
        col: u32,
        value: &crate::table::CellValue,
    ) -> Result<()>;

    /// 覆盖保存到新路径（`apply=copy` / 备份用）。
    fn save_as(&self, path: &Path) -> Result<()>;

    /// 保存回打开时的路径（`apply=inplace`，调用方应先写 .bak）。
    fn save_in_place(&self) -> Result<()>;
}

/// 默认实现：umya-spreadsheet。M0 阶段先占位，Spike 完成后填充。
pub struct UmyaWorkbook;

impl UmyaWorkbook {
    pub fn new() -> Self {
        Self
    }
}

impl Default for UmyaWorkbook {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkbookIO for UmyaWorkbook {
    fn open_template(&mut self, _path: &Path) -> Result<()> {
        Err(Error::NotImplemented(
            "WorkbookIO::open_template（umya Spike）",
        ))
    }

    fn read_source(&mut self, _src: &SourceSpec, _month_dir: &Path) -> Result<Table> {
        Err(Error::NotImplemented("WorkbookIO::read_source"))
    }

    fn write_cell(
        &mut self,
        _sheet: &str,
        _row: u32,
        _col: u32,
        _value: &crate::table::CellValue,
    ) -> Result<()> {
        Err(Error::NotImplemented("WorkbookIO::write_cell"))
    }

    fn save_as(&self, _path: &Path) -> Result<()> {
        Err(Error::NotImplemented("WorkbookIO::save_as"))
    }

    fn save_in_place(&self) -> Result<()> {
        Err(Error::NotImplemented("WorkbookIO::save_in_place"))
    }
}
