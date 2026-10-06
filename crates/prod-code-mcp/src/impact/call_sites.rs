/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use url::Url;

use crate::session::LspSession;

use super::diff::Change;
use super::pool::SessionPool;
use super::symbols::one_based;
use super::test_cmd::{file_language, rel};
use super::types::{CallSite, SignatureWarning, Symbol};

pub(crate) async fn discover_call_sites(
    session: &mut LspSession,
    root: &Path,
    sym: &Symbol,
) -> Vec<(String, u32, u32, u32, Option<String>)> {
    let mut sites = Vec::new();
    let abs = root.join(&sym.file);
    let Ok(uri) = Url::from_file_path(&abs).map(|u| u.to_string()) else {
        return sites;
    };
    let position = serde_json::json!({
        "line": sym.line.saturating_sub(1),
        "character": sym.col.saturating_sub(1)
    });

    if let Ok(serde_json::Value::Array(locs)) = session
        .query(
            &abs,
            "textDocument/references",
            serde_json::json!({
                "textDocument": { "uri": uri },
                "position": position,
                "context": { "includeDeclaration": false }
            }),
        )
        .await
    {
        let mut source_cache: HashMap<String, String> = HashMap::new();
        for loc in locs {
            let Some(loc_uri) = loc.get("uri").and_then(|u| u.as_str()) else {
                continue;
            };
            let rel_file = rel(root, loc_uri);
            if rel_file.starts_with('/') {
                continue;
            }
            if let Some(start) = loc.pointer("/range/start")
                && let Some(line) = one_based(start, "line")
                && let Some(col) = one_based(start, "character")
            {
                let source = source_cache.entry(rel_file.clone()).or_insert_with(|| {
                    std::fs::read_to_string(root.join(&rel_file)).unwrap_or_default()
                });
                let file_lang = file_language(root, &rel_file).unwrap_or("generic");
                let end_line = call_expression_end_line(source, line, col, &sym.name, file_lang)
                    .unwrap_or(line);
                sites.push((rel_file, line, col, end_line, None));
            }
        }
    }

    let prepare = "textDocument/prepareCallHierarchy";
    if let Ok(serde_json::Value::Array(items)) = session
        .query(
            &abs,
            prepare,
            serde_json::json!({ "textDocument": { "uri": uri }, "position": position }),
        )
        .await
    {
        for item in items {
            if let Ok(serde_json::Value::Array(edges)) = session
                .query(
                    &abs,
                    "callHierarchy/incomingCalls",
                    serde_json::json!({ "item": item }),
                )
                .await
            {
                for edge in edges {
                    let caller_name = edge
                        .pointer("/from/name")
                        .and_then(|n| n.as_str())
                        .map(ToString::to_string);
                    let caller_uri = edge.pointer("/from/uri").and_then(|u| u.as_str());
                    let caller_file = caller_uri.map(|u| rel(root, u));

                    if let Some(serde_json::Value::Array(ranges)) = edge.get("fromRanges") {
                        for range in ranges {
                            if let Some(rel_file) = &caller_file
                                && !rel_file.starts_with('/')
                                && let Some(start) = range.get("start")
                                && let Some(line) = one_based(start, "line")
                                && let Some(col) = one_based(start, "character")
                            {
                                let end_line = range
                                    .get("end")
                                    .and_then(|end| one_based(end, "line"))
                                    .unwrap_or(line);
                                sites.push((
                                    rel_file.clone(),
                                    line,
                                    col,
                                    end_line.max(line),
                                    caller_name.clone(),
                                ));
                            }
                        }
                    } else if let Some(rel_file) = caller_file
                        && !rel_file.starts_with('/')
                        && let Some(start) = edge
                            .pointer("/from/selectionRange/start")
                            .or_else(|| edge.pointer("/from/range/start"))
                        && let Some(line) = one_based(start, "line")
                        && let Some(col) = one_based(start, "character")
                    {
                        let source =
                            std::fs::read_to_string(root.join(&rel_file)).unwrap_or_default();
                        let file_lang = file_language(root, &rel_file).unwrap_or("generic");
                        let end_line =
                            call_expression_end_line(&source, line, col, &sym.name, file_lang)
                                .unwrap_or(line);
                        sites.push((rel_file, line, col, end_line, caller_name.clone()));
                    }
                }
            }
        }
    }

    let mut map: BTreeMap<(String, u32, u32), (u32, Option<String>)> = BTreeMap::new();
    for (file, line, col, end_line, caller) in sites {
        let entry = map
            .entry((file, line, col))
            .or_insert((end_line, caller.clone()));
        entry.0 = entry.0.max(end_line);
        if entry.1.is_none() && caller.is_some() {
            entry.1 = caller;
        }
    }

    map.into_iter()
        .map(|((file, line, col), (end_line, caller))| (file, line, col, end_line, caller))
        .collect()
}

pub(crate) fn lsp_position_to_byte_offset(text: &str, line: u32, character: u32) -> Option<usize> {
    if line == 0 || character == 0 {
        return None;
    }
    let mut offset = 0usize;
    for (index, raw_line) in text.split_inclusive('\n').enumerate() {
        if index as u32 + 1 != line {
            offset += raw_line.len();
            continue;
        }
        let line_text = raw_line
            .strip_suffix('\n')
            .unwrap_or(raw_line)
            .strip_suffix('\r')
            .unwrap_or_else(|| raw_line.strip_suffix('\n').unwrap_or(raw_line));
        let target = (character - 1) as usize;
        let mut units = 0usize;
        for (byte, ch) in line_text.char_indices() {
            if units == target {
                return Some(offset + byte);
            }
            let next = units + ch.len_utf16();
            if target < next {
                return None;
            }
            units = next;
        }
        return (units == target).then_some(offset + line_text.len());
    }
    None
}

pub(crate) fn call_expression_end_line(
    text: &str,
    line: u32,
    col: u32,
    name: &str,
    language: &str,
) -> Option<u32> {
    let start = lsp_position_to_byte_offset(text, line, col)?;
    let rest = text.get(start..)?;
    if !rest.starts_with(name) {
        return None;
    }
    let mut cursor = start + name.len();
    while text[cursor..]
        .chars()
        .next()
        .is_some_and(char::is_whitespace)
    {
        cursor += text[cursor..].chars().next()?.len_utf8();
    }
    if language == "rust" && text[cursor..].starts_with("::<") {
        cursor += 2;
    }
    if text[cursor..].starts_with('<') {
        let mut depth = 0usize;
        let mut close = None;
        for (offset, ch) in text[cursor..].char_indices() {
            match ch {
                '<' => depth += 1,
                '>' => {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        close = Some(cursor + offset + ch.len_utf8());
                        break;
                    }
                }
                _ => {}
            }
        }
        cursor = close?;
    }
    while text[cursor..]
        .chars()
        .next()
        .is_some_and(char::is_whitespace)
    {
        cursor += text[cursor..].chars().next()?.len_utf8();
    }
    if text.as_bytes().get(cursor) != Some(&b'(') {
        return None;
    }
    let close = crate::parameter_object::matching_bracket(text, cursor)?;
    Some(text[..=close].bytes().filter(|byte| *byte == b'\n').count() as u32 + 1)
}

pub(crate) async fn check_signature_warnings(
    session_pool: &mut SessionPool<'_>,
    root: &Path,
    changes: &BTreeMap<String, Change>,
    adjusted_signatures: Vec<(Symbol, String, String, u32, u32)>,
) -> Vec<SignatureWarning> {
    let mut signature_warnings: Vec<SignatureWarning> = Vec::new();
    for (sym, old_sig, new_sig, sig_start, sig_end) in adjusted_signatures {
        let sym_abs = root.join(&sym.file);
        let sites = match session_pool.session_for_file(&sym_abs).await {
            Ok(sym_session) => discover_call_sites(sym_session, root, &sym).await,
            Err(_) => Vec::new(),
        };
        let mut unadjusted = Vec::new();
        for (call_file, call_line, call_col, call_end_line, caller) in sites {
            if call_file == sym.file && call_line >= sig_start && call_line <= sig_end {
                continue;
            }
            let is_sibling = call_file != sym.file;
            let adjusted = match changes.get(&call_file) {
                None => false,
                Some(Change::Hunks(hunks)) => {
                    hunks.iter().any(|h| h.touches(call_line, call_end_line))
                }
                Some(Change::Unknown(_)) => true,
            };
            if !adjusted {
                unadjusted.push(CallSite {
                    file: call_file,
                    line: call_line,
                    col: call_col,
                    caller,
                    is_sibling,
                });
            }
        }
        if !unadjusted.is_empty() {
            unadjusted.sort_by(|a, b| {
                b.is_sibling
                    .cmp(&a.is_sibling)
                    .then_with(|| a.file.cmp(&b.file))
                    .then_with(|| a.line.cmp(&b.line))
                    .then_with(|| a.col.cmp(&b.col))
            });
            unadjusted.dedup();
            signature_warnings.push(SignatureWarning {
                symbol: sym,
                old_signature: old_sig,
                new_signature: new_sig,
                unadjusted_call_sites: unadjusted,
            });
        }
    }
    signature_warnings
}
