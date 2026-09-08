//! 工作表 XML 的最小 patch：只改目标 `<c>` 单元格的值，其余 XML 结构不动。
//!
//! 前提（本项目的模板形态）：数据行里目标列通常已有带样式的空单元格
//! （`<c r="E5" s="23"/>` 或 `<c r="E5" s="23"></c>`），因此主路径是：
//!   已有单元格 → 写/替换其 `<v>`；不存在该坐标时才插入一个新 `<c>`。
//! 数值按文本写入（整数/浮点 to_string）。
//!
//! 类型处理：模板目标列可能是表格软件“清空后保存”的文本型空单元格
//! （`<c … t="s"/>`），此时 `<v>` 里的数字会被当作共享字符串下标显示成乱码。
//! 因此写值/清空都会先把开标签里的 `t="…"` 去掉，让数值写成真正数值单元格。

/// 把 `writes`（单元格引用 → 值文本）应用到 worksheet XML 文本上。
/// `None` 表示清空该单元格（移除值、保留样式/位置）。
pub fn patch_cells(xml: &str, writes: &[(String, Option<String>)]) -> String {
    let mut out = xml.to_string();
    for (r#ref, value) in writes {
        out = patch_one(&out, r#ref, value.as_deref());
    }
    out
}

/// 去掉单元格开标签里的类型属性 `t="…"`（同时兼容 `t='…'`）。
/// 引擎现状只向目标格写数值或清空，数值格绝不能残留 `t="s"`
/// （否则 `<v>100</v>` 会被 Excel 一族当共享字符串下标 #100 显示）。
fn strip_type_attr(tag: &str) -> String {
    let mut out = String::with_capacity(tag.len());
    let mut rest = tag;
    loop {
        let Some(p) = rest.find(" t=") else {
            out.push_str(rest);
            return out;
        };
        let after_eq = &rest[p + 3..]; // 跳过 " t="
        let Some(quote) = after_eq.chars().next() else {
            out.push_str(rest);
            return out;
        };
        if quote != '"' && quote != '\'' {
            out.push_str(rest);
            return out;
        }
        let value = &after_eq[quote.len_utf8()..];
        let Some(end) = value.find(quote) else {
            out.push_str(rest);
            return out;
        };
        // 属性总长 = " t="(3) + 引号 + 值 + 引号。
        let attr_len = 3 + quote.len_utf8() + end + quote.len_utf8();
        out.push_str(&rest[..p]);
        rest = &rest[p + attr_len..];
    }
}

fn patch_one(xml: &str, cell_ref: &str, value: Option<&str>) -> String {
    let marker = format!("<c r=\"{cell_ref}\"");
    let Some(open) = xml.find(&marker) else {
        return match value {
            Some(v) => insert_new_cell(xml, cell_ref, v),
            // 单元格不存在时无需"清空"。
            None => xml.to_string(),
        };
    };

    // 定位该 <c ...> 的结束 '>'。
    let Some(rel_end) = xml[open..].find('>') else {
        return xml.to_string();
    };
    let tag_end = open + rel_end;
    let tag = &xml[open..tag_end];
    let is_self_closing = tag.ends_with('/');
    // 统一去掉类型属性：数值写为数值格；清空也不留下“空字符串型”残留格。
    let attrs = strip_type_attr(if is_self_closing {
        &tag[..tag.len() - 1]
    } else {
        tag
    });

    // 清空：把 <c ...>内容</c> 压成 <c .../>（保留样式/坐标）。
    let Some(value) = value else {
        let mut s = String::with_capacity(xml.len());
        s.push_str(&xml[..open]);
        s.push_str(&attrs);
        s.push_str("/>");
        if is_self_closing {
            s.push_str(&xml[tag_end + 1..]);
        } else {
            let body_start = tag_end + 1;
            let Some(rel_close) = xml[body_start..].find("</c>") else {
                return xml.to_string();
            };
            let body_end = body_start + rel_close;
            s.push_str(&xml[body_end + "</c>".len()..]);
        }
        return s;
    };

    if is_self_closing {
        // <c r="E5" s="23" t="s"/> → <c r="E5" s="23"><v>value</v></c>
        let mut s = String::with_capacity(xml.len() + value.len() + 16);
        s.push_str(&xml[..open]);
        s.push_str(&attrs);
        s.push('>');
        s.push_str(&format!("<v>{value}</v></c>"));
        s.push_str(&xml[tag_end + 1..]);
        return s;
    }

    // 非自闭合：正文从 '>' 之后开始。
    let body_start = tag_end + 1;
    let body_end = xml[body_start..]
        .find("</c>")
        .map(|e| body_start + e)
        .unwrap_or(xml.len());
    let mut s = String::with_capacity(xml.len() + value.len() + 8);
    s.push_str(&xml[..open]);
    s.push_str(&attrs);
    s.push('>');

    if let Some(vrel) = xml[body_start..body_end].find("<v>") {
        let vstart = body_start + vrel;
        let after_v = vstart + 3;
        if let Some(verel) = xml[after_v..body_end].find("</v>") {
            let vend = after_v + verel;
            s.push_str(&xml[body_start..vstart]);
            s.push_str("<v>");
            s.push_str(value);
            s.push_str(&xml[vend..]); // 从旧 </v> 起，含其后的正文/收尾
            return s;
        }
    }

    // 有 <c> 开标签但无 <v>：在 </c> 前插入 <v>。
    s.push_str(&xml[body_start..body_end]);
    s.push_str(&format!("<v>{value}</v>"));
    s.push_str(&xml[body_end..]);
    s
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
        let out = patch_cells(xml, &[("A1".into(), Some("99".into()))]);
        assert!(
            out.contains("<c r=\"A1\" s=\"1\"><v>99</v></c>"),
            "got: {out}"
        );
        assert!(out.contains("<c r=\"B1\" s=\"2\"><v>2</v></c>"));
    }

    #[test]
    fn fills_empty_cell_self_closing() {
        let xml = "<row r=\"3\"><c r=\"C3\" s=\"9\"/><c r=\"D3\"/></row>";
        let out = patch_cells(xml, &[("C3".into(), Some("1.5".into()))]);
        assert!(
            out.contains("<c r=\"C3\" s=\"9\"><v>1.5</v></c>"),
            "got: {out}"
        );
    }

    #[test]
    fn fills_empty_cell_open_close() {
        let xml = "<row r=\"3\"><c r=\"C3\" s=\"9\"></c></row>";
        let out = patch_cells(xml, &[("C3".into(), Some("1.5".into()))]);
        assert!(
            out.contains("<c r=\"C3\" s=\"9\"><v>1.5</v></c>"),
            "got: {out}"
        );
    }

    #[test]
    fn clears_existing_value() {
        let xml = "<row r=\"1\"><c r=\"A1\" s=\"3\"><v>13.68</v></c></row>";
        let out = patch_cells(xml, &[("A1".into(), None)]);
        assert!(out.contains("<c r=\"A1\" s=\"3\"/>"), "got: {out}");
        assert!(!out.contains("<v>"), "值应被清除: {out}");
    }

    #[test]
    fn writes_number_into_text_typed_cell_self_closing() {
        // 表格软件清空后保存的文本型空单元格：<c ... t="s"/>。
        let xml = "<row r=\"3\"><c r=\"J3\" s=\"75\" t=\"s\"/></row>";
        let out = patch_cells(xml, &[("J3".into(), Some("100".into()))]);
        assert!(
            out.contains("<c r=\"J3\" s=\"75\"><v>100</v></c>"),
            "got: {out}"
        );
        assert!(!out.contains("t=\"s\""), "写数值不应残留字符串类型: {out}");
    }

    #[test]
    fn writes_number_into_text_typed_cell_with_value() {
        // 文本型空单元格带旧下标值（模板常见的空串占位）。
        let xml = "<row r=\"3\"><c r=\"J3\" s=\"75\" t=\"s\"><v>32</v></c></row>";
        let out = patch_cells(xml, &[("J3".into(), Some("100".into()))]);
        assert!(
            out.contains("<c r=\"J3\" s=\"75\"><v>100</v></c>"),
            "got: {out}"
        );
        assert!(!out.contains("t=\"s\""), "got: {out}");
    }

    #[test]
    fn clears_text_typed_cell_without_leaving_string_type() {
        let xml = "<row r=\"3\"><c r=\"M3\" s=\"76\" t=\"s\"><v>32</v></c></row>";
        let out = patch_cells(xml, &[("M3".into(), None)]);
        assert!(out.contains("<c r=\"M3\" s=\"76\"/>"), "got: {out}");
        assert!(!out.contains("t=\"s\""), "清空不应残留字符串类型: {out}");
        assert!(!out.contains("<v>"), "got: {out}");
    }

    #[test]
    fn clears_text_typed_cell_self_closing() {
        let xml = "<row r=\"3\"><c r=\"M3\" s=\"76\" t=\"s\"/></row>";
        let out = patch_cells(xml, &[("M3".into(), None)]);
        assert!(out.contains("<c r=\"M3\" s=\"76\"/>"), "got: {out}");
        assert!(!out.contains("t=\"s\""), "got: {out}");
    }

    #[test]
    fn inserts_new_cell_into_row() {
        let xml = "<sheetData><row r=\"5\"><c r=\"A5\"><v>1</v></c></row></sheetData>";
        let out = patch_cells(xml, &[("E5".into(), Some("42".into()))]);
        assert!(out.contains("<c r=\"E5\"><v>42</v></c>"), "got: {out}");
        assert!(out.contains("<c r=\"A5\"><v>1</v></c>"));
    }
}
