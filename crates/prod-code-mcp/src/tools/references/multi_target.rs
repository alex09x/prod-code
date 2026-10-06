/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use url::Url;

use crate::tools::execute_lsp_query;
use crate::tools::symbols::{MAX_SCANNED_FILES, names_word, source_files};

/// The most places in the checkout asked for their definition when looking for a use.
pub(crate) const MAX_USES_ASKED: usize = 40;

/// The most places in one file asked: a file no target compiles answers none of them, and a name
/// that resolves elsewhere in a file means the same elsewhere through the rest of it.
pub(crate) const MAX_USES_ASKED_PER_FILE: usize = 2;

/// Whether `text` writes `name(` with `name` as a whole word, a call or a declaration.
pub(crate) fn writes_call(text: &str, name: &str) -> bool {
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    text.match_indices(name).any(|(at, _)| {
        !text[..at].chars().next_back().is_some_and(ident)
            && text[at + name.len()..].trim_start().starts_with('(')
    })
}

pub(crate) fn add_locations(
    locations: &serde_json::Value,
    seen_locs: &mut HashSet<(String, u64, u64)>,
    seen_files: &mut HashSet<PathBuf>,
    all_locs: &mut Vec<serde_json::Value>,
) {
    if let Some(arr) = locations.as_array() {
        for loc in arr {
            let uri = loc
                .get("uri")
                .or_else(|| loc.get("targetUri"))
                .and_then(|u| u.as_str())
                .unwrap_or("");
            let start_line = loc
                .pointer("/range/start/line")
                .or_else(|| loc.pointer("/targetRange/start/line"))
                .and_then(|l| l.as_u64())
                .unwrap_or(0)
                + 1;
            let start_col = loc
                .pointer("/range/start/character")
                .or_else(|| loc.pointer("/targetRange/start/character"))
                .and_then(|c| c.as_u64())
                .unwrap_or(0)
                + 1;
            if seen_locs.insert((uri.to_string(), start_line, start_col)) {
                all_locs.push(loc.clone());
                if let Ok(parsed) = Url::parse(uri)
                    && let Ok(path) = parsed.to_file_path()
                {
                    let norm = std::fs::canonicalize(&path).unwrap_or(path);
                    seen_files.insert(norm);
                }
            }
        }
    }
}

pub(crate) fn engine_can_reference(caller_engine: Option<&str>, decl_engine: &str) -> bool {
    match (caller_engine, decl_engine) {
        (Some(c), d) if c == d => true,
        (Some("astro" | "svelte" | "vue" | "html" | "javascript"), "typescript") => true,
        (Some("astro" | "svelte" | "vue" | "html" | "typescript"), "javascript") => true,
        (Some("cpp"), "c") | (Some("c"), "cpp") => true,
        _ => false,
    }
}

pub(crate) fn imports_declaration(text: &str, decl_path: &Path, name: &str) -> bool {
    let file_stem = decl_path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    for line in text.lines() {
        let trimmed = line.trim();
        if (trimmed.starts_with("import ") || trimmed.starts_with("from "))
            && trimmed.contains(name)
        {
            if file_stem.is_empty() || trimmed.contains(file_stem) {
                return true;
            }
        }
        if (trimmed.contains("require(") || trimmed.contains("import("))
            && trimmed.contains(file_stem)
        {
            return true;
        }
    }
    false
}

/// Collects references across multiple feature-gated targets, shared modules, or separate checkouts (#783).
/// Scans candidate source files across the workspace that contain `name`. When a candidate use resolves
/// to the target declaration via `textDocument/definition`, queries references from that target's context
/// to capture all call sites in live adapters and secondary targets. Also collects unindexed call sites
/// for warning reporting.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn collect_multi_target_references(
    remote: SocketAddr,
    workspace_root: &Path,
    declaration: &Path,
    decl_line: u32,
    decl_col: u32,
    name: &str,
    include_decl: bool,
    initial_was_empty: bool,
    seen_locs: &mut HashSet<(String, u64, u64)>,
    seen_files: &mut HashSet<PathBuf>,
    all_locs: &mut Vec<serde_json::Value>,
    unindexed_callers: &mut Vec<PathBuf>,
) -> Option<String> {
    let language = crate::sync::engine_for_file(declaration)?;
    let declared = declaration.to_string_lossy().into_owned();
    let wanted: Vec<char> = name.chars().collect();
    let is_name = |c: Option<&char>| c.is_some_and(|c| c.is_alphanumeric() || *c == '_');
    let mut asked = 0usize;
    let mut checkout_use_note = None;
    let candidates = source_files(workspace_root)
        .filter(|path| engine_can_reference(crate::sync::engine_for_file(path), language))
        .take(MAX_SCANNED_FILES);
    for path in candidates {
        let is_declaration_file = path == declaration;
        let norm_path = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if seen_files.contains(&norm_path) && (!is_declaration_file || !initial_was_empty) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if !names_word(&text, name) {
            continue;
        }
        let Ok(uri) = Url::from_file_path(&path) else {
            continue;
        };
        let uri_str = uri.to_string();
        let wanted_len = wanted.len();
        let file_uses: Vec<(usize, usize)> = text
            .lines()
            .enumerate()
            .flat_map(|(index, text_line)| {
                let is_declaration_line = is_declaration_file && index as u32 + 1 == decl_line;
                let chars: Vec<char> = text_line.chars().collect();
                (0..chars.len())
                    .filter(|&c| {
                        if !chars[c..].starts_with(&wanted)
                            || (c > 0 && is_name(chars.get(c - 1)))
                            || is_name(chars.get(c + wanted_len))
                        {
                            return false;
                        }
                        if is_declaration_line {
                            let match_start = c as u32 + 1;
                            let match_end = match_start + wanted_len as u32;
                            if match_start <= decl_col && decl_col <= match_end {
                                return false;
                            }
                        }
                        true
                    })
                    .map(move |c| (index, c))
                    .collect::<Vec<_>>()
            })
            .collect();

        for (index, col) in file_uses.into_iter().take(MAX_USES_ASKED_PER_FILE) {
            asked += 1;
            if asked > MAX_USES_ASKED {
                return checkout_use_note;
            }
            let params = serde_json::json!({
                "textDocument": { "uri": &uri_str },
                "position": { "line": index, "character": col }
            });
            let found_def = execute_lsp_query(
                remote,
                workspace_root,
                &path,
                "textDocument/definition",
                params,
            )
            .await
            .ok();

            if let Some(ref found) = found_def
                && definition_is(found, &declared, decl_line.saturating_sub(1))
            {
                seen_files.insert(norm_path.clone());
                let use_line = (index + 1) as u64;
                let use_col = (col + 1) as u64;

                let ref_params = serde_json::json!({
                    "textDocument": { "uri": &uri_str },
                    "position": { "line": index, "character": col },
                    "context": { "includeDeclaration": include_decl }
                });
                if let Ok(target_refs) = execute_lsp_query(
                    remote,
                    workspace_root,
                    &path,
                    "textDocument/references",
                    ref_params,
                )
                .await
                {
                    add_locations(&target_refs, seen_locs, seen_files, all_locs);
                }

                if seen_locs.insert((uri_str.clone(), use_line, use_col)) {
                    all_locs.push(serde_json::json!({
                        "uri": &uri_str,
                        "range": {
                            "start": { "line": index, "character": col },
                            "end": { "line": index, "character": col + wanted_len }
                        }
                    }));
                }

                if checkout_use_note.is_none() && initial_was_empty {
                    let is_external = crate::remote_fs::is_external(
                        workspace_root,
                        &declaration.to_string_lossy(),
                    );
                    let origin = if is_external {
                        "at the dependency's own declaration the server found none"
                    } else {
                        "at the declaration the server found none"
                    };
                    checkout_use_note = Some(format!(
                        "(asked from a use of `{name}` in the checkout, {}:{use_line}:{use_col}: {origin})\n",
                        path.strip_prefix(workspace_root).unwrap_or(&path).display()
                    ));
                }
                break;
            } else {
                if let Some(ref found) = found_def
                    && (found.as_array().is_some_and(|a| !a.is_empty()) || found.is_object())
                {
                    // The name means another item in this file.
                    break;
                }
                let has_no_def = found_def
                    .as_ref()
                    .is_none_or(|f| f.as_array().is_none_or(|a| a.is_empty()) && !f.is_object());
                if !is_declaration_file && has_no_def && writes_call(&text, name) {
                    if !unindexed_callers.contains(&path) {
                        unindexed_callers.push(path.clone());
                    }
                    if imports_declaration(&text, declaration, name) {
                        seen_files.insert(norm_path.clone());
                        let use_line = (index + 1) as u64;
                        let use_col = (col + 1) as u64;
                        if seen_locs.insert((uri_str.clone(), use_line, use_col)) {
                            all_locs.push(serde_json::json!({
                                "uri": &uri_str,
                                "range": {
                                    "start": { "line": index, "character": col },
                                    "end": { "line": index, "character": col + wanted_len }
                                }
                            }));
                        }
                    }
                }
            }
        }
    }
    checkout_use_note
}

/// Whether a `textDocument/definition` answer (a location, a list of them, or of links) names
/// 0-based `line` of the file at `path`.
pub(crate) fn definition_is(found: &serde_json::Value, path: &str, line: u32) -> bool {
    let locations: Vec<&serde_json::Value> = match found {
        serde_json::Value::Array(all) => all.iter().collect(),
        serde_json::Value::Object(_) => vec![found],
        _ => Vec::new(),
    };
    locations.iter().any(|location| {
        let uri = location
            .get("uri")
            .or_else(|| location.get("targetUri"))
            .and_then(|u| u.as_str())
            .unwrap_or("");
        let start = location
            .pointer("/range/start/line")
            .or_else(|| location.pointer("/targetSelectionRange/start/line"))
            .and_then(|l| l.as_u64());
        crate::remote_fs::uri_to_path(uri) == path && start == Some(line as u64)
    })
}
