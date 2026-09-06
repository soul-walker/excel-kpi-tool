//! 引用改名传播：数据源 id / 列别名 / 派生 id / 视图列名 / 聚合指标 id / 参数名被改名后，
//! 把方案里引用这些名字的字符串同步改写（from、表达式、where、parse/merge 等）。
//!
//! DSL 约定“表引用一律带命名空间前缀”，因此引用只会以两种形态出现：
//! - 表 id（数据源/派生）作为整串等于 `.from`，或以 `{id}.` 前缀出现；
//! - 列 alias / 派生视图列名 / 参数名作为 `.` 后的成员（前缀是表 id 或“参数”）。
//!
//! 改写只作用于符合上述结构的“词”，不做无界全串替换；单引号字符串字面量
//! （DSL 字符串）内部不改写。检测按“同一序号前后对照”，行/列数量对不上时
//! 宁可跳过成员级改名，避免删除+新增被误判成改名。

use std::collections::HashMap;

use serde_json::Value;

type CountKey = (&'static str, String, String);
type Counts = HashMap<CountKey, usize>;

/// 表 id 改名（kind 用于日志文案）。
struct TableRename {
    old: String,
    new: String,
    kind: &'static str,
}

/// 成员改名：prefix 是“最终”归属前缀（表 id 或“参数”）。
struct MemberRename {
    prefix: String,
    old: String,
    new: String,
    kind: &'static str,
}

#[derive(Default)]
struct RenameDiff {
    table: Vec<TableRename>,
    member: Vec<MemberRename>,
}

impl RenameDiff {
    fn is_empty(&self) -> bool {
        self.table.is_empty() && self.member.is_empty()
    }
}

fn arr(v: Option<&Value>) -> Option<&Vec<Value>> {
    v.and_then(|x| x.as_array())
}

fn field_of(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string()
}

fn push_member(diff: &mut RenameDiff, prefix: &str, old: String, new: String, kind: &'static str) {
    if !old.is_empty() && old != new {
        diff.member.push(MemberRename {
            prefix: prefix.to_string(),
            old,
            new,
            kind,
        });
    }
}

/// 同下标成对比较，收集改名。
fn detect(before: &Value, after: &Value) -> RenameDiff {
    let mut diff = RenameDiff::default();
    let (Some(b), Some(a)) = (before.get("scheme"), after.get("scheme")) else {
        return diff;
    };

    // 参数名（影响 参数.名字 引用）。
    if let (Some(bp), Some(ap)) = (arr(b.get("parameters")), arr(a.get("parameters")))
        && bp.len() == ap.len()
    {
        for (brow, arow) in bp.iter().zip(ap.iter()) {
            let old = field_of(brow, "name");
            let new = field_of(arow, "name");
            push_member(&mut diff, "参数", old, new, "参数名");
        }
    }

    // 数据源：id + 列 alias（成员前缀 = 该源最终 id）。
    if let (Some(bsrc), Some(asrc)) = (arr(b.get("sources")), arr(a.get("sources")))
        && bsrc.len() == asrc.len()
    {
        for (bs, as_) in bsrc.iter().zip(asrc.iter()) {
            let old_id = field_of(bs, "id");
            let new_id = field_of(as_, "id");
            if !old_id.is_empty() && old_id != new_id {
                diff.table.push(TableRename {
                    old: old_id,
                    new: new_id.clone(),
                    kind: "数据源 id",
                });
            }
            if let (Some(bc), Some(ac)) = (arr(bs.get("columns")), arr(as_.get("columns")))
                && bc.len() == ac.len()
            {
                for (bc_, ac_) in bc.iter().zip(ac.iter()) {
                    let old_alias = field_of(bc_, "alias");
                    let new_alias = field_of(ac_, "alias");
                    push_member(&mut diff, &new_id, old_alias, new_alias, "列 alias");
                }
            }
        }
    }

    // 派生：id + 视图列名（派生视图也是“表”，下游以 {派生id}.列名 引用它）。
    if let (Some(bd), Some(ad)) = (arr(b.get("derived")), arr(a.get("derived")))
        && bd.len() == ad.len()
    {
        for (bd_, ad_) in bd.iter().zip(ad.iter()) {
            let old_id = field_of(bd_, "id");
            let new_id = field_of(ad_, "id");
            if !old_id.is_empty() && old_id != new_id {
                diff.table.push(TableRename {
                    old: old_id,
                    new: new_id.clone(),
                    kind: "派生 id",
                });
            }
            if let (Some(bc), Some(ac)) = (arr(bd_.get("columns")), arr(ad_.get("columns")))
                && bc.len() == ac.len()
            {
                for (bc_, ac_) in bc.iter().zip(ac.iter()) {
                    let old_name = field_of(bc_, "name");
                    let new_name = field_of(ac_, "name");
                    push_member(&mut diff, &new_id, old_name, new_name, "视图列名");
                }
            }
        }
    }

    // 聚合指标：id 改名（表达式里以 `聚合.{id}` 引用）。
    if let (Some(before_aggs), Some(after_aggs)) =
        (arr(b.get("aggregates")), arr(a.get("aggregates")))
        && before_aggs.len() == after_aggs.len()
    {
        for (old_item, new_item) in before_aggs.iter().zip(after_aggs.iter()) {
            let old_id = field_of(old_item, "id");
            let new_id = field_of(new_item, "id");
            push_member(&mut diff, "聚合", old_id, new_id, "聚合指标");
        }
    }
    diff
}

// ---------- 文本改写 ----------

fn is_word(c: char) -> bool {
    !matches!(
        c,
        ' ' | '\t'
            | '\n'
            | '\r'
            | '.'
            | ','
            | ';'
            | ':'
            | '/'
            | '\\'
            | '~'
            | '`'
            | '!'
            | '%'
            | '^'
            | '&'
            | '*'
            | '('
            | ')'
            | '='
            | '+'
            | '['
            | ']'
            | '{'
            | '}'
            | '<'
            | '>'
            | '|'
            | '#'
            | '@'
            | '"'
            | '\''
            | '-'
    )
}

/// 改写 `{old}.` 前缀 → `{new}.`（跳过单引号字面量）；返回 (新文本, 替换次数)。
fn replace_table_prefix(text: &str, old: &str, new: &str) -> (String, usize) {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let old_len = old.chars().count();
    let mut out = String::with_capacity(n);
    let mut count = 0;
    let mut i = 0;
    let mut in_literal = false;
    while i < n {
        let c = chars[i];
        if in_literal {
            out.push(c);
            if c == '\'' {
                in_literal = false;
            }
            i += 1;
            continue;
        }
        if c == '\'' {
            in_literal = true;
            out.push(c);
            i += 1;
            continue;
        }
        if is_word(c) {
            let start = i;
            while i < n && is_word(chars[i]) {
                i += 1;
            }
            let wlen = i - start;
            if i < n && chars[i] == '.' && wlen == old_len {
                let w: String = chars[start..i].iter().collect();
                if w == old {
                    out.push_str(new);
                    out.push('.');
                    i += 1; // 吃掉 '.'
                    count += 1;
                    continue;
                }
            }
            out.extend(&chars[start..i]);
            continue;
        }
        out.push(c);
        i += 1;
    }
    (out, count)
}

/// 改写成员：`{prefix}.{old}` → `{prefix}.{new}`（跳过单引号字面量）。
fn replace_member_token(text: &str, prefix: &str, old: &str, new: &str) -> (String, usize) {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let old_len = old.chars().count();
    let prefix_len = prefix.chars().count();
    let mut out = String::with_capacity(n);
    let mut count = 0;
    let mut i = 0;
    let mut in_literal = false;
    while i < n {
        let c = chars[i];
        if in_literal {
            out.push(c);
            if c == '\'' {
                in_literal = false;
            }
            i += 1;
            continue;
        }
        if c == '\'' {
            in_literal = true;
            out.push(c);
            i += 1;
            continue;
        }
        if is_word(c) {
            let start = i;
            while i < n && is_word(chars[i]) {
                i += 1;
            }
            let wlen = i - start;
            if wlen == old_len && start > prefix_len && chars[start - 1] == '.' {
                let w: String = chars[start..i].iter().collect();
                if w == old {
                    let ps = start - prefix_len - 1;
                    let prev: String = chars[ps..start - 1].iter().collect();
                    if prev == prefix {
                        out.push_str(new);
                        count += 1;
                        continue;
                    }
                }
            }
            out.extend(&chars[start..i]);
            continue;
        }
        out.push(c);
        i += 1;
    }
    (out, count)
}

// ---------- 整方案遍历 ----------

fn count_bump(counts: &mut Counts, kind: &'static str, old: &str, new: &str, n: usize) {
    if n > 0 {
        *counts
            .entry((kind, old.to_string(), new.to_string()))
            .or_insert(0) += n;
    }
}

/// 对所有“表达式/过滤”字符串节点应用同一改写。
fn visit_expr_fields(after: &mut Value, visit: &mut dyn FnMut(&mut Value)) {
    let Some(scheme) = after.get_mut("scheme") else {
        return;
    };
    if let Some(ds) = scheme.get_mut("derived").and_then(|x| x.as_array_mut()) {
        for d in ds {
            if let Some(w) = d.get_mut("where") {
                visit(w);
            }
            if let Some(cols) = d.get_mut("columns").and_then(|x| x.as_array_mut()) {
                for c in cols {
                    if let Some(e) = c.get_mut("expr") {
                        visit(e);
                    }
                }
            }
        }
    }
    if let Some(items) = scheme.get_mut("aggregates").and_then(|x| x.as_array_mut()) {
        for item in items {
            if let Some(w) = item.get_mut("where") {
                visit(w);
            }
            visit(&mut item["expr"]);
        }
    }
    if let Some(gs) = scheme.get_mut("groups").and_then(|x| x.as_array_mut()) {
        for g in gs {
            for key in ["fields", "overrides"] {
                if let Some(fs) = g.get_mut(key).and_then(|x| x.as_array_mut()) {
                    for fe in fs {
                        if let Some(e) = fe.get_mut("expr") {
                            visit(e);
                        }
                    }
                }
            }
        }
    }
}

/// 整串 = 表 id 的 from 字段改写。
fn rewrite_from_fields(after: &mut Value, diff: &RenameDiff, counts: &mut Counts) {
    let Some(scheme) = after.get_mut("scheme") else {
        return;
    };
    for t in &diff.table {
        let old = &t.old;
        let new = &t.new;
        let mut matched = 0usize;
        let mut fix = |node: &mut Value| {
            if node.as_str().is_some_and(|s| s == old.as_str()) {
                *node = Value::String(new.clone());
                matched += 1;
            }
        };
        for d in scheme
            .get_mut("derived")
            .and_then(|x| x.as_array_mut())
            .into_iter()
            .flatten()
        {
            fix(&mut d["from"]);
        }
        for a in scheme
            .get_mut("aggregates")
            .and_then(|x| x.as_array_mut())
            .into_iter()
            .flatten()
        {
            fix(&mut a["from"]);
        }
        if let Some(outs) = scheme.get_mut("outputs").and_then(|x| x.as_array_mut()) {
            for o in outs {
                let Some(nf) = o.get_mut("kind").and_then(|k| k.get_mut("new_file")) else {
                    continue;
                };
                fix(&mut nf["from"]);
            }
        }
        count_bump(counts, t.kind, old, new, matched);
    }
}

/// 表达式文本：先整条表 id 前缀，后整条成员 token。
fn rewrite_exprs(after: &mut Value, diff: &RenameDiff, counts: &mut Counts) {
    for t in &diff.table {
        let (old, new, kind) = (&t.old, &t.new, t.kind);
        visit_expr_fields(after, &mut |node: &mut Value| {
            let Some(text) = node.as_str() else { return };
            let (out, n) = replace_table_prefix(text, old, new);
            if n > 0 {
                *node = Value::String(out);
                count_bump(counts, kind, old, new, n);
            }
        });
    }
    for m in &diff.member {
        let (prefix, old, new, kind) = (&m.prefix, &m.old, &m.new, m.kind);
        visit_expr_fields(after, &mut |node: &mut Value| {
            let Some(text) = node.as_str() else { return };
            let (out, n) = replace_member_token(text, prefix, old, new);
            if n > 0 {
                *node = Value::String(out);
                count_bump(counts, kind, old, new, n);
            }
        });
    }
}

/// 把 list 中整项等于旧成员的值替换成新值（用于 parse/merge/group_by 等裸名上下文）。
fn rewrite_bare_list(
    list: &mut [Value],
    prefix: &str,
    diff: &RenameDiff,
    counts: &mut Counts,
    kind: &'static str,
) {
    let map: Vec<(&str, &str)> = diff
        .member
        .iter()
        .filter(|m| m.prefix == prefix)
        .map(|m| (m.old.as_str(), m.new.as_str()))
        .collect();
    if map.is_empty() {
        return;
    }
    for (old, new) in &map {
        let n = list
            .iter()
            .filter(|item| item.as_str().is_some_and(|s| s == *old))
            .count();
        if n > 0 {
            count_bump(counts, kind, old, new, n);
        }
    }
    for item in list.iter_mut() {
        let Some(s) = item.as_str() else { continue };
        if let Some((_, new)) = map.iter().find(|(old, _)| *old == s) {
            *item = Value::String((*new).to_string());
        }
    }
}

/// 裸成员上下文改写（parse 列表 / merge.on / aggregate.group_by / new_file 列）。
fn rewrite_bare_contexts(after: &mut Value, diff: &RenameDiff, counts: &mut Counts) {
    let Some(scheme) = after.get_mut("scheme") else {
        return;
    };
    // sources[i].parse.fill_down / continue_record_on（本源列 alias）。
    if let Some(srcs) = scheme.get_mut("sources").and_then(|x| x.as_array_mut()) {
        for s in srcs {
            let final_id = field_of(s, "id");
            let Some(parse) = s.get_mut("parse") else {
                continue;
            };
            for key in ["fill_down", "continue_record_on"] {
                if let Some(list) = parse.get_mut(key).and_then(|x| x.as_array_mut()) {
                    rewrite_bare_list(list, &final_id, diff, counts, "解析列");
                }
            }
        }
    }
    // derived.merge.on（derived.from 表的列 alias）。
    if let Some(ds) = scheme.get_mut("derived").and_then(|x| x.as_array_mut()) {
        for d in ds {
            let from = field_of(d, "from");
            if let Some(merge) = d.get_mut("merge")
                && let Some(on) = merge.get_mut("on").and_then(|x| x.as_array_mut())
            {
                rewrite_bare_list(on, &from, diff, counts, "合并列");
            }
        }
    }
    // aggregate.group_by（aggregate.from 表的列 alias）。
    if let Some(items) = scheme.get_mut("aggregates").and_then(|x| x.as_array_mut()) {
        for item in items {
            let from = field_of(item, "from");
            let gb = item.get("group_by").and_then(|x| x.as_str()).unwrap_or("");
            let target: Option<(String, String)> = diff
                .member
                .iter()
                .find(|m| m.prefix == from && m.old == gb)
                .map(|m| (m.old.clone(), m.new.clone()));
            if let Some((old, new)) = target {
                item["group_by"] = Value::String(new.clone());
                count_bump(counts, "聚合分组列", &old, &new, 1);
            }
        }
    }
    // output.new_file.columns[].field（new_file.from 的列/视图列名）。
    if let Some(outs) = scheme.get_mut("outputs").and_then(|x| x.as_array_mut()) {
        for o in outs {
            let Some(nf) = o.get_mut("kind").and_then(|k| k.get_mut("new_file")) else {
                continue;
            };
            let from = field_of(nf, "from");
            if let Some(cols) = nf.get_mut("columns").and_then(|x| x.as_array_mut()) {
                let map: Vec<(String, String)> = diff
                    .member
                    .iter()
                    .filter(|m| m.prefix == from)
                    .map(|m| (m.old.clone(), m.new.clone()))
                    .collect();
                for (old, new) in &map {
                    let n = cols
                        .iter()
                        .filter(|c| c["field"].as_str().is_some_and(|s| s == old))
                        .count();
                    if n > 0 {
                        count_bump(counts, "输出列", old, new, n);
                    }
                }
                for c in cols.iter_mut() {
                    let Some(s) = c["field"].as_str() else {
                        continue;
                    };
                    if let Some((_, new)) = map.iter().find(|(old, _)| *old == s) {
                        c["field"] = Value::String(new.clone());
                    }
                }
            }
        }
    }
}

fn apply(after: &mut Value, diff: &RenameDiff) -> Vec<String> {
    let mut counts: Counts = HashMap::new();
    rewrite_from_fields(after, diff, &mut counts);
    rewrite_exprs(after, diff, &mut counts);
    rewrite_bare_contexts(after, diff, &mut counts);

    // —— 组装日志 ——
    let mut logs = Vec::new();
    for t in &diff.table {
        let n = counts
            .get(&(t.kind, t.old.clone(), t.new.clone()))
            .copied()
            .unwrap_or(0);
        logs.push(format!(
            "{}：{} → {}（同步 {} 处引用）",
            t.kind, t.old, t.new, n
        ));
    }
    for m in &diff.member {
        let n = counts
            .get(&(m.kind, m.old.clone(), m.new.clone()))
            .copied()
            .unwrap_or(0);
        if n > 0 {
            logs.push(format!(
                "{}：{} → {}（前缀 {}，同步 {} 处引用）",
                m.kind, m.old, m.new, m.prefix, n
            ));
        }
    }
    logs
}

/// 入口：before 是改写前快照，after 原地同步引用；返回日志（空 = 无改名）。
pub fn propagate(before: &Value, after: &mut Value) -> Vec<String> {
    let diff = detect(before, after);
    if diff.is_empty() {
        return Vec::new();
    }
    apply(after, &diff)
}

/// 传播后校验：derived/aggregates 的 from 必须指向现有数据源或派生。
/// 供保存/校验在“写完控件 → 改名传播”之后调用，仍能给出友好报错。
pub fn check_table_refs(raw: &Value) -> Result<(), String> {
    let Some(scheme) = raw.get("scheme") else {
        return Ok(());
    };
    let mut tables: Vec<&str> = Vec::new();
    if let Some(srcs) = scheme.get("sources").and_then(|x| x.as_array()) {
        tables.extend(
            srcs.iter()
                .filter_map(|s| s.get("id").and_then(|x| x.as_str())),
        );
    }
    if let Some(ds) = scheme.get("derived").and_then(|x| x.as_array()) {
        tables.extend(
            ds.iter()
                .filter_map(|d| d.get("id").and_then(|x| x.as_str())),
        );
    }
    let ok = |from: &str| !from.is_empty() && tables.contains(&from);
    if let Some(ds) = scheme.get("derived").and_then(|x| x.as_array()) {
        for d in ds {
            let id = d.get("id").and_then(|x| x.as_str()).unwrap_or("?");
            let from = d.get("from").and_then(|x| x.as_str()).unwrap_or("");
            if !ok(from) {
                return Err(format!("派生[{id}]：来源 from 必须指向已有数据源或派生"));
            }
        }
    }
    if let Some(items) = scheme.get("aggregates").and_then(|x| x.as_array()) {
        for item in items {
            let id = item.get("id").and_then(|x| x.as_str()).unwrap_or("?");
            let from = item.get("from").and_then(|x| x.as_str()).unwrap_or("");
            if !ok(from) {
                return Err(format!("聚合[{id}]：来源 from 必须指向已有数据源或派生"));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn table_prefix_only_exact_word() {
        // 只改词后紧跟 . 的整词，`qingjia2.` 与引号内文本不动。
        let (out, n) = replace_table_prefix(
            "qingjia.天数 + qingjia2.天数 == 'x.qingjia.y'",
            "qingjia",
            "请假表",
        );
        assert_eq!(n, 1);
        assert_eq!(out, "请假表.天数 + qingjia2.天数 == 'x.qingjia.y'");
    }

    #[test]
    fn member_token_only_under_prefix() {
        let (out, n) =
            replace_member_token("qingjia.天数 + keshi.天数", "qingjia", "天数", "请假天数");
        assert_eq!(n, 1);
        assert_eq!(out, "qingjia.请假天数 + keshi.天数");
    }

    #[test]
    fn propagate_renames_source_id_and_alias() {
        let before = json!({
            "scheme": {
                "sources": [
                    { "id": "qingjia", "file": "a.xlsx", "sheet": "S", "columns": [
                        { "header": "天数", "alias": "天数", "value_type": "decimal" },
                        { "header": "姓名", "alias": "姓名", "value_type": "text", "key": true }
                    ] }
                ],
                "derived": [
                    {
                        "id": "代课", "from": "qingjia",
                        "where": "qingjia.替课 != ''",
                        "columns": [
                            { "name": "姓名", "expr": "GROUP_BY(qingjia.姓名)" },
                            { "name": "总天数", "expr": "SUM(qingjia.天数)" }
                        ]
                    }
                ]
            }
        });
        let mut after = before.clone();
        after["scheme"]["sources"][0]["id"] = json!("请假表");
        after["scheme"]["sources"][0]["columns"][0]["alias"] = json!("请假天数");
        let logs = propagate(&before, &mut after);
        assert!(!logs.is_empty());
        let d = &after["scheme"]["derived"][0];
        assert_eq!(d["from"], "请假表");
        assert_eq!(d["columns"][1]["expr"], "SUM(请假表.请假天数)");
        assert_eq!(d["columns"][0]["expr"], "GROUP_BY(请假表.姓名)");
        assert_eq!(d["where"], "请假表.替课 != ''");
    }

    #[test]
    fn propagate_renames_derived_id() {
        let before = json!({
            "scheme": {
                "sources": [{ "id": "qingjia", "columns": [] }],
                "derived": [
                    { "id": "bukesh", "from": "qingjia", "columns": [
                        { "name": "节数", "expr": "SUM(qingjia.节数)" }
                    ] },
                    { "id": "bks", "from": "bukesh", "where": "bukesh.节数 > 0", "columns": [] }
                ]
            }
        });
        let mut after = before.clone();
        after["scheme"]["derived"][0]["id"] = json!("补课");
        let logs = propagate(&before, &mut after);
        assert!(!logs.is_empty());
        assert_eq!(after["scheme"]["derived"][1]["from"], "补课");
        assert_eq!(after["scheme"]["derived"][1]["where"], "补课.节数 > 0");
    }

    #[test]
    fn propagate_renames_agg_id() {
        let before = json!({
            "scheme": {
                "aggregates": [
                    { "id": "请假超总天数", "from": "qingjia", "group_by": "姓名", "expr": "SUM(qingjia.天数)" },
                    { "id": "请假超额", "from": "qingjia", "group_by": "姓名", "expr": "聚合.请假超总天数 >= 参数.请假超过天数" }
                ],
                "groups": [{
                    "id": "fxp",
                    "fields": [
                        { "name": "课时S", "expr": "IF(聚合.请假超额, 0, keshi.课时 ?? 0)" }
                    ],
                    "overrides": []
                }]
            }
        });
        let mut after = before.clone();
        after["scheme"]["aggregates"][0]["id"] = json!("请假总天数");
        let logs = propagate(&before, &mut after);
        assert!(!logs.is_empty());
        assert_eq!(
            after["scheme"]["aggregates"][1]["expr"],
            "聚合.请假总天数 >= 参数.请假超过天数"
        );
        assert_eq!(
            after["scheme"]["groups"][0]["fields"][0]["expr"],
            "IF(聚合.请假超额, 0, keshi.课时 ?? 0)"
        );
    }

    #[test]
    fn propagate_renames_param() {
        let before = json!({
            "scheme": {
                "parameters": [{ "name": "补课时费", "value_type": "integer", "value": 10 }],
                "derived": [{
                    "id": "代课", "from": "qingjia",
                    "columns": [{ "name": "费用", "expr": "bukesh.节数 * 参数.补课时费" }]
                }]
            }
        });
        let mut after = before.clone();
        after["scheme"]["parameters"][0]["name"] = json!("代课单价");
        let logs = propagate(&before, &mut after);
        assert!(!logs.is_empty());
        assert_eq!(
            after["scheme"]["derived"][0]["columns"][0]["expr"],
            "bukesh.节数 * 参数.代课单价"
        );
    }
}
