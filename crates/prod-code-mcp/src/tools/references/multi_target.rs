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

use super::across::{
    ImportMatch, definition_is, engine_can_reference, imports_declaration, is_occurrence_shadowed,
    mask_executable_code, masked_code_string, writes_call,
};
use super::{
    definition_in_file, definition_in_other_workspace_file, swift_shares_or_imports_target,
};
use crate::tools::execute_lsp_query;
use crate::tools::symbols::{
    MAX_SCANNED_FILES, is_in_nested_git_worktree, names_word, source_files,
};

/// The most time multi-target references discovery can take before returning accumulated references.
pub(crate) const MULTI_TARGET_REF_BUDGET: std::time::Duration = std::time::Duration::from_secs(8);

/// The most places in the checkout asked for their definition when looking for a use.
pub(crate) const MAX_USES_ASKED: usize = 12;

/// The most places in one file asked: a file no target compiles answers none of them, and a name
/// that resolves elsewhere in a file means the same elsewhere through the rest of it.
pub(crate) const MAX_USES_ASKED_PER_FILE: usize = 2;

pub(crate) fn add_locations(
    workspace_root: &Path,
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
            let parsed_path = Url::parse(uri).ok().and_then(|u| u.to_file_path().ok());
            if let Some(ref path) = parsed_path
                && is_in_nested_git_worktree(workspace_root, path)
            {
                continue;
            }
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
                if let Some(path) = parsed_path {
                    let norm = std::fs::canonicalize(&path).unwrap_or(path);
                    seen_files.insert(norm);
                }
            }
        }
    }
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
    let deadline = tokio::time::Instant::now() + MULTI_TARGET_REF_BUDGET;
    let is_unexported_go =
        language == "go" && name.chars().next().is_some_and(|c| c.is_ascii_lowercase());
    let decl_parent = declaration.parent();
    let candidates = source_files(workspace_root)
        .filter(|path| engine_can_reference(crate::sync::engine_for_file(path), language))
        .take(MAX_SCANNED_FILES);
    for path in candidates {
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        if is_unexported_go && path.parent() != decl_parent {
            continue;
        }
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
        };
        let Ok(uri) = Url::from_file_path(&path) else {
            continue;
        };
        let uri_str = uri.to_string();
        let wanted_len = wanted.len();
        let code_mask = mask_executable_code(&text, &path);
        let masked_code = masked_code_string(&text, &code_mask);
        let text_lines: Vec<&str> = text.lines().collect();
        let mut line_byte_start = 0usize;
        let mut file_uses: Vec<(usize, usize, usize)> = Vec::new();
        for (index, raw_line) in text.split('\n').enumerate() {
            let text_line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
            let is_declaration_line = is_declaration_file && index as u32 + 1 == decl_line;
            let chars: Vec<(usize, char)> = text_line.char_indices().collect();
            for c in 0..chars.len() {
                if c + wanted_len > chars.len() {
                    break;
                }
                let byte_off = line_byte_start + chars[c].0;
                if !code_mask
                    .get(byte_off..byte_off + name.len())
                    .is_some_and(|slice| slice.iter().all(|&b| b))
                {
                    continue;
                }
                let matches_word = (0..wanted_len).all(|k| chars[c + k].1 == wanted[k]);
                if !matches_word
                    || (c > 0 && is_name(Some(&chars[c - 1].1)))
                    || is_name(chars.get(c + wanted_len).map(|(_, ch)| ch))
                {
                    continue;
                }
                if is_declaration_line {
                    let match_start = c as u32 + 1;
                    let match_end = match_start + wanted_len as u32;
                    if match_start <= decl_col && decl_col <= match_end {
                        continue;
                    }
                }
                file_uses.push((index, c, byte_off));
            }
            line_byte_start += raw_line.len() + 1;
        }

        if file_uses.is_empty() {
            continue;
        }

        let non_shadowed: Vec<(usize, usize)> = file_uses
            .iter()
            .filter(|&&(u_idx, _, u_off)| {
                !is_occurrence_shadowed(&masked_code, &text_lines, u_idx, u_off, name)
            })
            .map(|&(u_idx, u_col, _)| (u_idx, u_col))
            .collect();

        // Markup/component template files (Astro, Svelte, Vue, HTML) importing a TypeScript or
        // JavaScript module are not indexed by `typescript-language-server`: record their uses
        // directly when they import `declaration`, without burning the LSP query budget (#1071).
        let file_engine = crate::sync::engine_for_file(&path);
        if !is_declaration_file && matches!(file_engine, Some("astro" | "svelte" | "vue" | "html"))
        {
            match imports_declaration(&text, &path, declaration, name) {
                ImportMatch::Proven => {
                    if !non_shadowed.is_empty() {
                        seen_files.insert(norm_path);
                        record_file_uses(&uri_str, &non_shadowed, wanted_len, seen_locs, all_locs);
                    }
                }
                ImportMatch::Uncertain => {
                    if !unindexed_callers.contains(&path) {
                        unindexed_callers.push(path.clone());
                    }
                }
                ImportMatch::None => {}
            }
            continue;
        }

        let is_swift = path.extension().and_then(|e| e.to_str()) == Some("swift")
            && declaration.extension().and_then(|e| e.to_str()) == Some("swift");

        for &(index, col, byte_off) in file_uses.iter().take(MAX_USES_ASKED_PER_FILE) {
            if tokio::time::Instant::now() >= deadline {
                return checkout_use_note;
            }
            asked += 1;
            if asked > MAX_USES_ASKED {
                return checkout_use_note;
            }
            let params = serde_json::json!({
                "textDocument": { "uri": &uri_str },
                "position": { "line": index, "character": col }
            });
            let found_def = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                execute_lsp_query(
                    remote,
                    workspace_root,
                    &path,
                    "textDocument/definition",
                    params,
                ),
            )
            .await
            .ok()
            .and_then(|r| r.ok());

            let def_matches = found_def.as_ref().is_some_and(|found| {
                definition_is(found, &declared, decl_line.saturating_sub(1))
                    || (is_swift && definition_in_file(found, &declared))
            });
            if def_matches {
                seen_files.insert(norm_path.clone());
                let use_line = (index + 1) as u64;
                let use_col = (col + 1) as u64;

                let ref_params = serde_json::json!({
                    "textDocument": { "uri": &uri_str },
                    "position": { "line": index, "character": col },
                    "context": { "includeDeclaration": include_decl }
                });
                if let Ok(Ok(target_refs)) = tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    execute_lsp_query(
                        remote,
                        workspace_root,
                        &path,
                        "textDocument/references",
                        ref_params,
                    ),
                )
                .await
                {
                    add_locations(
                        workspace_root,
                        &target_refs,
                        seen_locs,
                        seen_files,
                        all_locs,
                    );
                }

                let uses_to_record: &[(usize, usize)] = if is_swift && !non_shadowed.is_empty() {
                    &non_shadowed
                } else {
                    &[(index, col)]
                };
                record_file_uses(&uri_str, uses_to_record, wanted_len, seen_locs, all_locs);

                // Same-file Swift uses are the declaration target's own (#1135); a Rust use in
                // the declaring file (a recursive call) is still where the answer came from.
                if checkout_use_note.is_none()
                    && initial_was_empty
                    && !(is_declaration_file && is_swift)
                {
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
                let import_match = imports_declaration(&text, &path, declaration, name);
                let has_proven_import = import_match == ImportMatch::Proven;
                let has_any_import = import_match != ImportMatch::None;
                let has_no_def = found_def
                    .as_ref()
                    .is_none_or(|f| f.as_array().is_none_or(|a| a.is_empty()) && !f.is_object());
                let def_in_other_ws_file = found_def.as_ref().is_some_and(|found| {
                    !has_no_def
                        && definition_in_other_workspace_file(found, workspace_root, &declared)
                });
                if is_swift
                    && !def_in_other_ws_file
                    && (is_declaration_file
                        || swift_shares_or_imports_target(&path, declaration, has_any_import))
                {
                    if has_no_def {
                        asked = asked.saturating_sub(1);
                    }
                    if !non_shadowed.is_empty() {
                        seen_files.insert(norm_path.clone());
                        record_file_uses(&uri_str, &non_shadowed, wanted_len, seen_locs, all_locs);
                    }
                    break;
                }
                if let Some(ref found) = found_def
                    && (found.as_array().is_some_and(|a| !a.is_empty()) || found.is_object())
                {
                    // The name means another item in this file.
                    break;
                }
                if !is_declaration_file
                    && has_no_def
                    && (has_any_import || writes_call(&masked_code, name))
                {
                    let is_own_decl =
                        is_occurrence_shadowed(&masked_code, &text_lines, index, byte_off, name);
                    if is_own_decl && !has_proven_import {
                        break;
                    }
                    if !unindexed_callers.contains(&path) {
                        unindexed_callers.push(path.clone());
                    }
                    if has_proven_import && !is_own_decl {
                        seen_files.insert(norm_path.clone());
                        record_file_uses(
                            &uri_str,
                            &[(index, col)],
                            wanted_len,
                            seen_locs,
                            all_locs,
                        );
                    }
                }
            }
        }
    }
    checkout_use_note
}

fn record_file_uses(
    uri_str: &str,
    uses: &[(usize, usize)],
    wanted_len: usize,
    seen_locs: &mut HashSet<(String, u64, u64)>,
    all_locs: &mut Vec<serde_json::Value>,
) {
    for &(u_idx, u_col) in uses {
        let use_line = (u_idx + 1) as u64;
        let use_col = (u_col + 1) as u64;
        if seen_locs.insert((uri_str.to_string(), use_line, use_col)) {
            all_locs.push(serde_json::json!({
                "uri": uri_str,
                "range": {
                    "start": { "line": u_idx, "character": u_col },
                    "end": { "line": u_idx, "character": u_col + wanted_len }
                }
            }));
        }
    }
}
