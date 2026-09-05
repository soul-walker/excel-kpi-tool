//! 工作表 XML 的最小 patch：只改目标 `<c>` 单元格的值，其余 XML 结构不动。
//!
//! 前提（本项目的模板形态）：数据行里目标列通常已有带样式的空单元格
//! （`<c r="E5" s="23"/>` 或 `<c r="E5" s="23"></c>`），因此主路径是：
//!   已有单元格 → 写/替换其 `<v>`；不存在该坐标时才插入一个新 `<c>`。
//! 数值按文本写入（整数/浮点 to_string）。

/// 把 `writes`（单元格引用 → 值文本）应用到 worksheet XML 文本上。
pub fn patch_cells(xml: &str, writes: &[(String, String)]) -> String {
    let mut out = xml.to_string();
    for (r#ref, value) in writes {
        out = patch_one(&out, r#ref, value);
    }
    out
}

fn patch_one(xml: &str, cell_ref: &str, value: &str) -> String {
    let marker = format!("<c r=\"{cell_ref}\"");
    let Some(open) = xml.find(&marker) else {
        return insert_new_cell(xml, cell_ref, value);
    };

    // 定位该 <c ...> 的结束 '>'。
    let Some(rel_end) = xml[open..].find('>') else {
        return xml.to_string();
    };
    let tag_end = open + rel_end;
    let tag = &xml[open..tag_end];

    if tag.ends_with('/') {
        // <c r="E5" s="23"/> → <c r="E5" s="23"><v>value</v></c>
        let mut s = String::with_capacity(xml.len() + value.len() + 16);
        s.push_str(&xml[..tag_end - 1]);
        s.push('>');
        s.push_str(&format!("<v>{value}</v></c>"));
        s.push_str(&xml[tag_end + 1..]);
        return s;
    }

    // 非自闭合：查找 <v> 或 </c>
    let body_start = tag_end + 1;
    let body_end = xml[body_start..]
        .find("</c>")
        .map(|e| body_start + e)
        .unwrap_or(xml.len());

    if let Some(vrel) = xml[body_start..body_end].find("<v>") {
        let vstart = body_start + vrel;
        let after_v = vstart + 3;
        if let Some(verel) = xml[after_v..body_end].find("</v>") {
            let vend = after_v + verel;
            let mut s = String::with_capacity(xml.len() + value.len());
            s.push_str(&xml[..vstart + 3]);
            s.push_str(value);
            s.push_str(&xml[vend..]);
            return s;
        }
    }

    // 有 <c> 开标签但无 <v>：在 </c> 前插入 <v>。
    if body_end < xml.len() {
        let mut s = String::with_capacity(xml.len() + value.len() + 8);
        s.push_str(&xml[..body_end]);
        s.push_str(&format!("<v>{value}</v>"));
        s.push_str(&xml[body_end..]);
        return s;
    }

    xml.to_string()
}

/// 插入全新的空样式单元格（拼到目标行内，顺序不影响解析）。
fn insert_new_cell(xml: &str, cell_ref: &str, value: &str) -> String {
    let Some(row_start) = cell_ref.find(|c: char| !c.is_ascii_alphabetic()) else {
        return xml.to_string();
    };
    let row = &cell_ref[row_start..];
    let row_marker = format!("<row r=\"{row}\"");
    let Some(row_open) = xml.find(&row_marker) else {
        return xml.to_string();
    };
    let Some(rel) = xml[row_open..].find('>') else {
        return xml.to_string();
    };
    let row_tag_end = row_open + rel;
    let inserted = format!("<c r=\"{cell_ref}\"><v>{value}</v></c>");
    let mut s = String::with_capacity(xml.len() + inserted.len());
    s.push_str(&xml[..row_tag_end + 1]);
    s.push_str(&inserted);
    s.push_str(&xml[row_tag_end + 1..]);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_existing_value() {
        let xml =
            "<row r=\"1\"><c r=\"A1\" s=\"1\"><v>1</v></c><c r=\"B1\" s=\"2\"><v>2</v></c></row>";
        let out = patch_cells(xml, &[("A1".into(), "99".into())]);
        assert!(
            out.contains("<c r=\"A1\" s=\"1\"><v>99</v></c>"),
            "got: {out}"
        );
        assert!(out.contains("<c r=\"B1\" s=\"2\"><v>2</v></c>"));
    }

    #[test]
    fn fills_empty_cell_self_closing() {
        let xml = "<row r=\"3\"><c r=\"C3\" s=\"9\"/><c r=\"D3\"/></row>";
        let out = patch_cells(xml, &[("C3".into(), "1.5".into())]);
        assert!(
            out.contains("<c r=\"C3\" s=\"9\"><v>1.5</v></c>"),
            "got: {out}"
        );
    }

    #[test]
    fn fills_empty_cell_open_close() {
        let xml = "<row r=\"3\"><c r=\"C3\" s=\"9\"></c></row>";
        let out = patch_cells(xml, &[("C3".into(), "1.5".into())]);
        assert!(
            out.contains("<c r=\"C3\" s=\"9\"><v>1.5</v></c>"),
            "got: {out}"
        );
    }

    #[test]
    fn inserts_new_cell_into_row() {
        let xml = "<sheetData><row r=\"5\"><c r=\"A5\"><v>1</v></c></row></sheetData>";
        let out = patch_cells(xml, &[("E5".into(), "42".into())]);
        assert!(out.contains("<c r=\"E5\"><v>42</v></c>"), "got: {out}");
        assert!(out.contains("<c r=\"A5\"><v>1</v></c>"));
    }
}
