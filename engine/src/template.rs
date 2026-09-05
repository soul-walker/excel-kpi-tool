//! 模板回写：在 zip 层面对工作簿做**最小 patch**。
//!
//! 思路（M0 决策）：不整簿重建 → 只改写目标 worksheet XML 里的若干 `<c>` 单元格，
//! 其余部件（styles/合并/筛选/其它 sheet/打印设置等）**原样保留**。
//! 需要知道 sheet 名 → worksheet XML 条目 的对应关系（读 workbook.xml + rels）。

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

use crate::error::{Error, Result};
use crate::xmlpatch;

/// 一次回写的编辑单元。
#[derive(Debug, Clone)]
pub struct SheetEdit {
    pub sheet: String,
    /// (单元格引用, 值文本)。值文本 None = 清空该单元格（移除值、保留样式/坐标）。
    pub cells: Vec<(String, Option<String>)>,
}

fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("{name}=");
    let start = tag.find(&needle)? + needle.len();
    let rest = &tag[start..];
    let q = rest.find('"')? + 1;
    let tail = &rest[q..];
    let end = tail.find('"')?;
    Some(&tail[..end])
}

/// 读取 zip 内所有条目到内存（模板文件都不大）。
fn read_all_entries(src: &Path) -> Result<Vec<(String, Vec<u8>)>> {
    let file = File::open(src)?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| Error::Runtime(format!("打开 zip 失败: {e}")))?;
    let mut out = Vec::with_capacity(archive.len());
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| Error::Runtime(format!("读取 zip 条目失败: {e}")))?;
        let name = entry.name().to_string();
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf)?;
        out.push((name, buf));
    }
    Ok(out)
}

fn text_of(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// 建立 sheet 名 → zip 条目名 的映射。
fn sheet_entry_map(entries: &[(String, Vec<u8>)]) -> Result<HashMap<String, String>> {
    let wb_entry = entries
        .iter()
        .find(|(n, _)| n == "xl/workbook.xml")
        .ok_or_else(|| Error::Runtime("zip 缺少 xl/workbook.xml".into()))?;
    let rels_entry = entries
        .iter()
        .find(|(n, _)| n == "xl/_rels/workbook.xml.rels")
        .ok_or_else(|| Error::Runtime("zip 缺少 workbook.xml.rels".into()))?;

    let wb = text_of(&wb_entry.1);
    let rels = text_of(&rels_entry.1);

    // rels: Id -> 条目名
    let mut rid_entry: HashMap<String, String> = HashMap::new();
    let mut pos = 0;
    while let Some(start) = rels[pos..].find("<Relationship") {
        let tag_start = pos + start;
        let rel_end = rels[tag_start..]
            .find('>')
            .map(|e| tag_start + e)
            .unwrap_or(rels.len());
        let tag = &rels[tag_start..rel_end];
        pos = rel_end + 1;
        let id = attr(tag, "Id").unwrap_or("");
        let ty = attr(tag, "Type").unwrap_or("");
        let target = attr(tag, "Target").unwrap_or("");
        if !id.is_empty() && ty.ends_with("/worksheet") && !target.is_empty() {
            let entry = if let Some(stripped) = target.strip_prefix('/') {
                stripped.to_string()
            } else {
                format!("xl/{target}")
            };
            rid_entry.insert(id.to_string(), entry);
        }
    }

    // workbook: sheet name -> r:id
    let mut map: HashMap<String, String> = HashMap::new();
    let sheets_open = wb.find("<sheets>").unwrap_or(0) + "<sheets>".len();
    let sheets_close = wb[sheets_open..]
        .find("</sheets>")
        .map(|e| sheets_open + e)
        .unwrap_or(wb.len());
    let sheets_xml = &wb[sheets_open..sheets_close];
    let mut pos = 0;
    while let Some(start) = sheets_xml[pos..].find("<sheet ") {
        let tag_start = pos + start;
        let tag_end = sheets_xml[tag_start..]
            .find('>')
            .map(|e| tag_start + e)
            .unwrap_or(sheets_xml.len());
        let tag = &sheets_xml[tag_start..tag_end];
        pos = tag_end + 1;
        let name = attr(tag, "name").unwrap_or("");
        let rid = attr(tag, "r:id").unwrap_or("");
        if !name.is_empty()
            && !rid.is_empty()
            && let Some(entry) = rid_entry.get(rid)
        {
            map.insert(name.to_string(), entry.clone());
        }
    }
    Ok(map)
}

/// 对 src 做最小 patch 并写出到 dst；返回被改写的条目名列表。
pub fn patch_workbook(src: &Path, dst: &Path, edits: &[SheetEdit]) -> Result<Vec<String>> {
    let mut entries = read_all_entries(src)?;
    let map = sheet_entry_map(&entries)?;
    let mut index_of: HashMap<String, usize> = HashMap::new();
    for (i, (name, _)) in entries.iter().enumerate() {
        index_of.insert(name.clone(), i);
    }

    let mut changed = Vec::new();
    for edit in edits {
        if edit.cells.is_empty() {
            continue;
        }
        let entry = map.get(&edit.sheet).ok_or_else(|| {
            Error::Runtime(format!("找不到 sheet {} 对应的 worksheet 条目", edit.sheet))
        })?;
        let idx = *index_of
            .get(entry.as_str())
            .ok_or_else(|| Error::Runtime(format!("zip 缺少条目 {entry}")))?;
        let xml = text_of(&entries[idx].1);
        let out = xmlpatch::patch_cells(&xml, &edit.cells);
        entries[idx].1 = out.into_bytes();
        changed.push(entry.clone());
    }

    // 写出新 zip（未改动的条目原样复制）。
    let file = File::create(dst)?;
    let mut zw = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, bytes) in &entries {
        zw.start_file(name, opts)
            .map_err(|e| Error::Runtime(format!("写入 zip 条目 {name} 失败: {e}")))?;
        zw.write_all(bytes)?;
    }
    zw.finish()
        .map_err(|e| Error::Runtime(format!("收尾 zip 失败: {e}")))?;
    Ok(changed)
}
