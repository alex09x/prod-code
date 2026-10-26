/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::net::SocketAddr;
use std::path::Path;

pub(crate) fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// The line span (1-based, inclusive) of the smallest declaration containing `line`, and the
/// name the analyzer gives it.
pub fn span_at(symbols: &serde_json::Value, line: u32) -> Option<(String, u32, u32)> {
    fn walk(nodes: &[serde_json::Value], line: u32, best: &mut Option<(String, u32, u32)>) {
        for node in nodes {
            let range = node
                .get("range")
                .or_else(|| node.get("location").and_then(|l| l.get("range")));
            if let Some(range) = range
                && let (Some(s), Some(e)) = (
                    range.pointer("/start/line").and_then(|l| l.as_u64()),
                    range.pointer("/end/line").and_then(|l| l.as_u64()),
                )
            {
                let (s, e) = (s as u32 + 1, e as u32 + 1);
                let name = node
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or_default()
                    .to_string();
                if s <= line && line <= e && best.as_ref().is_none_or(|(_, bs, be)| e - s < be - bs)
                {
                    *best = Some((name, s, e));
                }
            }
            if let Some(children) = node.get("children").and_then(|c| c.as_array()) {
                walk(children, line, best);
            }
        }
    }
    let mut best = None;
    walk(symbols.as_array().map(|a| a.as_slice())?, line, &mut best);
    best
}

/// The first line of the item once its doc comment and attributes are counted as part of it.
///
/// The analyzer's range starts at `fn`/`struct`; a declaration without its `///` and `#[…]`
/// is a different declaration.
pub fn with_doc_comment(text: &str, start: u32) -> u32 {
    super::rust_syntax::item_start(text, start, ra_ap_syntax::Edition::CURRENT)
}

/// `text` without lines `start..=end`, and those lines on their own.
///
/// Exactly those lines and no others: the caller adjusts every position below the hole by the
/// number of lines removed, and a cut that also tidied the blank lines around it would make
/// that arithmetic a lie. Whatever blank line is left over is the formatter's business.
pub fn cut(text: &str, start: u32, end: u32) -> (String, String) {
    let s = crate::signature::offset_of(text, start, 1).unwrap_or(text.len());
    let e = crate::signature::offset_of(text, end.saturating_add(1), 1).unwrap_or(text.len());
    let item = text[s..e].strip_suffix('\n').unwrap_or(&text[s..e]);
    let item = item.strip_suffix('\r').unwrap_or(item).to_string();
    let mut out = text.to_string();
    out.replace_range(s..e, "");
    (out, item)
}

/// `item` appended to `text`, with exactly one blank line between them.
pub fn append_item(text: &str, item: &str) -> String {
    let mut out = text.to_string();
    if !out.ends_with('\n') {
        out.push('\n');
    }
    if !out.ends_with("\n\n") {
        out.push('\n');
    }
    out.push_str(item);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Move references use the same UTF-16 columns and CRLF line breaks as every LSP query.
pub(crate) fn offset_of(text: &str, line: u32, col: u32) -> Option<usize> {
    crate::signature::offset_of(text, line, col)
}

fn owner_token_pos(
    text: &str,
    impl_at: usize,
    impl_open: usize,
    owner: &str,
) -> Option<(u32, u32)> {
    let header = &text[impl_at..impl_open];
    let head = header.split("where").next().unwrap_or(header);
    let is_id = |c: char| c.is_alphanumeric() || c == '_';
    let mut found = None;
    for (idx, _) in head.match_indices(owner) {
        let before_ok = !head[..idx].chars().next_back().is_some_and(is_id);
        let after_ok = !head[idx + owner.len()..].chars().next().is_some_and(is_id);
        if before_ok && after_ok {
            found = Some(impl_at + idx);
        }
    }
    found.and_then(|off| crate::signature::position_at(text, off).ok())
}

fn impl_constraints_key(header: &str, owner: &str) -> String {
    let compact: String = header.split_whitespace().collect::<Vec<_>>().join(" ");
    let is_id = |c: char| c.is_alphanumeric() || c == '_';
    if let Some((idx, _)) = compact.match_indices(owner).find(|(i, _)| {
        !compact[..*i].chars().next_back().is_some_and(is_id)
            && !compact[*i + owner.len()..]
                .chars()
                .next()
                .is_some_and(is_id)
    }) {
        let prefix = compact[..idx].trim_end();
        let path_start = prefix
            .rfind(|c: char| !is_id(c) && c != ':')
            .map_or(0, |p| p + 1);
        format!("{}{}", &prefix[..path_start], &compact[idx..])
    } else {
        compact
    }
}

async fn resolve_type_def(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    (line, col): (u32, u32),
) -> Option<(String, u64, u64)> {
    let uri = url::Url::from_file_path(file).ok()?.to_string();
    let val = crate::tools::execute_lsp_query(
        remote,
        root,
        file,
        "textDocument/definition",
        serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": line.saturating_sub(1), "character": col.saturating_sub(1) },
        }),
    )
    .await
    .ok()?;
    let item = if let Some(arr) = val.as_array() {
        arr.first()?
    } else {
        &val
    };
    let target_uri = item
        .get("targetUri")
        .or_else(|| item.get("uri"))
        .and_then(|v| v.as_str())?
        .to_string();
    let start = item
        .pointer("/targetSelectionRange/start")
        .or_else(|| item.pointer("/range/start"))?;
    Some((
        target_uri,
        start.get("line")?.as_u64()?,
        start.get("character")?.as_u64()?,
    ))
}

/// Returns the index in `impl_blocks(target_text)` of the inherent `impl` block whose resolved
/// owner declaration and `impl` constraints match the source `impl` block at `impl_at..impl_open`.
pub(crate) async fn verified_target_impl_index(
    remote: SocketAddr,
    root: &Path,
    source_file: &Path,
    source_text: &str,
    (impl_at, impl_open): (usize, usize),
    target_file: &Path,
    target_text: &str,
    owner: &str,
) -> Option<usize> {
    let src_pos = owner_token_pos(source_text, impl_at, impl_open, owner)?;
    let src_def = resolve_type_def(remote, root, source_file, src_pos).await?;
    let src_key = impl_constraints_key(&source_text[impl_at..impl_open], owner);
    let mut matched = Vec::new();
    for (idx, (ty, t_at, t_open, _)) in crate::extract_field::impl_blocks(target_text)
        .into_iter()
        .enumerate()
    {
        if ty != owner || target_text[t_at..t_open].contains(" for ") {
            continue;
        }
        if impl_constraints_key(&target_text[t_at..t_open], owner) != src_key {
            continue;
        }
        let Some(t_pos) = owner_token_pos(target_text, t_at, t_open, owner) else {
            continue;
        };
        if resolve_type_def(remote, root, target_file, t_pos)
            .await
            .as_ref()
            == Some(&src_def)
        {
            matched.push(idx);
        }
    }
    (matched.len() == 1).then(|| matched[0])
}

/// Cuts an inherent method at `start..=decl_end` (with declaration header at `decl_start`) from
/// `source_text`'s `impl` block and inserts it into `target_text`'s verified `impl` block.
#[allow(clippy::too_many_arguments)]
pub(crate) fn move_inherent_method(
    source_text: &str,
    target_text: &str,
    target_impl_idx: Option<usize>,
    (impl_at, impl_open, impl_close): (usize, usize, usize),
    start: u32,
    decl_start: u32,
    decl_end: u32,
) -> (String, String, String, Vec<String>) {
    let (source_cut, raw_item) = cut(source_text, start, decl_end);
    let span_start = crate::signature::offset_of(source_text, start, 1).unwrap_or(impl_open + 1);
    let span_end =
        crate::signature::offset_of(source_text, decl_end + 1, 1).unwrap_or(source_text.len());
    let (cut_s, cut_e) = crate::move_method::cut_from_impl(
        source_text,
        (impl_at, impl_open, impl_close),
        (span_start, span_end),
    );
    let source_new = if (cut_s, cut_e) != (span_start, span_end) {
        let mut s = String::with_capacity(source_text.len());
        s.push_str(&source_text[..cut_s]);
        s.push_str(&source_text[cut_e..]);
        s
    } else {
        source_cut
    };

    let decl_rel = decl_start.saturating_sub(start) as usize;
    let mut item_lines: Vec<String> = raw_item.lines().map(str::to_string).collect();
    if let Some(sig_line) = item_lines.get_mut(decl_rel) {
        let trimmed = sig_line.trim_start();
        if !trimmed.starts_with("pub")
            && (trimmed.starts_with("fn ")
                || trimmed.starts_with("async fn ")
                || trimmed.starts_with("const fn ")
                || trimmed.starts_with("unsafe fn "))
        {
            let indent_len = sig_line.len() - trimmed.len();
            sig_line.insert_str(indent_len, "pub(crate) ");
        }
    }
    let item = item_lines.join("\n");
    let (mut target_with_imports, carried) =
        super::imports::carry_imports(source_text, &item, target_text);
    let target_block = target_impl_idx.and_then(|idx| {
        crate::extract_field::impl_blocks(&target_with_imports)
            .into_iter()
            .nth(idx)
    });
    let target_new = if let Some((_, _, _, target_close)) = target_block {
        let before = target_with_imports[..target_close].trim_end_matches([' ', '\t']);
        let lead = if before.ends_with("{\n") || before.ends_with('{') {
            ""
        } else {
            "\n"
        };
        let mut out = String::with_capacity(target_with_imports.len() + item.len() + 4);
        out.push_str(before);
        out.push_str(lead);
        out.push_str(item.trim_end());
        out.push('\n');
        out.push_str(&target_with_imports[target_close..]);
        out
    } else {
        let header = source_text[impl_at..=impl_open].trim();
        if !target_with_imports.contains("use super::*;") {
            target_with_imports = super::imports::add_import(&target_with_imports, "use super::*;");
        }
        let block = format!("{header}\n{}\n}}", item.trim_end());
        if target_with_imports.trim().is_empty() {
            format!("{block}\n")
        } else {
            append_item(&target_with_imports, &block)
        }
    };
    (source_new, target_new, item, carried)
}
