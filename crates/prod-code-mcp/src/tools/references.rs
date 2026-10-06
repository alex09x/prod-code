/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Finding and filtering references across projects and multi-target call sites.

use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use url::Url;

use super::symbols::{MAX_SCANNED_FILES, names_word, source_files};
use super::{build_swift_index, execute_lsp_query, execute_tool, resolve_file_path};
use crate::protocol::McpToolCallResult;

pub(crate) async fn handle_references(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let line = args
        .get("line")
        .and_then(|v| v.as_u64())
        .context("Missing 'line' argument")? as u32;
    let character = args
        .get("character")
        .and_then(|v| v.as_u64())
        .context("Missing 'character' argument")? as u32;
    let include_decl = args
        .get("include_declarations")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let file_path = resolve_file_path(workspace_root, path_str);
    let file_uri = Url::from_file_path(&file_path)
        .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {:?}", file_path))?
        .to_string();
    // A position on no name has no references: "No references found" read as "nothing uses
    // this" when it meant "you pointed at nothing" (#373).
    let line_text = position_line(remote, workspace_root, &file_path, line).await;
    if let Some(text) = &line_text
        && name_at(text, character).is_none()
    {
        anyhow::bail!(
            "{path_str}:{line}:{character} is on no name; the line reads `{}`. Give the position of \
             a use or a declaration, or pass `symbol`",
            text.trim()
        );
    }
    let params = serde_json::json!({
        "textDocument": { "uri": file_uri },
        "position": { "line": line.saturating_sub(1), "character": character.saturating_sub(1) },
        "context": { "includeDeclaration": include_decl }
    });
    let mut res = execute_lsp_query(
        remote,
        workspace_root,
        &file_path,
        "textDocument/references",
        params.clone(),
    )
    .await?;
    let mut out = String::new();

    let mut all_locs = Vec::new();
    let mut seen_locs = HashSet::new();
    let mut seen_files = HashSet::new();
    let mut unindexed_callers = Vec::new();

    add_locations(&res, &mut seen_locs, &mut seen_files, &mut all_locs);
    let initial_was_empty = all_locs.is_empty();
    if !initial_was_empty {
        seen_files.insert(std::fs::canonicalize(&file_path).unwrap_or_else(|_| file_path.clone()));
    }

    let name_opt = line_text.as_deref().and_then(|t| name_at(t, character));

    if let Some(ref name) = name_opt
        && let Some(note) = collect_multi_target_references(
            remote,
            workspace_root,
            &file_path,
            line,
            character,
            name,
            include_decl,
            initial_was_empty,
            &mut seen_locs,
            &mut seen_files,
            &mut all_locs,
            &mut unindexed_callers,
        )
        .await
    {
        out.push_str(&note);
    }

    if all_locs.is_empty()
        && let Some((built, note)) = build_swift_index(remote, workspace_root, &file_path).await
    {
        out.push_str(&note);
        out.push('\n');
        if built {
            res = execute_lsp_query(
                remote,
                workspace_root,
                &file_path,
                "textDocument/references",
                params,
            )
            .await?;
            add_locations(&res, &mut seen_locs, &mut seen_files, &mut all_locs);
        }
    }

    // An empty answer says what the position stood on, so a position one line off (an
    // attribute above the function) shows as such (#373).
    let stood_on = line_text
        .as_deref()
        .and_then(|text| Some((name_at(text, character)?, text.trim())))
        .map(|(name, text)| format!("\n(the position is on `{name}`; the line reads `{text}`)"))
        .unwrap_or_default();

    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
        .unwrap_or(0);

    if all_locs.is_empty() {
        out.push_str("No references found.");
        out.push_str(&stood_on);
    } else {
        let total = all_locs.len();
        out.push_str(&format!("Found {total} reference(s):\n"));
        let display_count = if limit > 0 { total.min(limit) } else { total };
        for loc in &all_locs[..display_count] {
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
            out.push_str(&format!("  • {uri}:{start_line}:{start_col}\n"));
        }
        if limit > 0 && total > limit {
            out.push_str(&format!(
                "  ... (display capped at first {display_count} of {total} references; pass `limit: 0` to display all)\n"
            ));
        }

        if total > 1 {
            let sym_name = args
                .get("symbol")
                .and_then(|v| v.as_str())
                .or(name_opt.as_deref());
            let sym_desc = match sym_name {
                Some(s) => format!("`{s}`"),
                None => "this symbol".to_string(),
            };
            out.push_str(&format!(
                "\n💡 Refactoring Tip: For workspace-wide renames or signature updates to {sym_desc}, prefer `code_rename` or `code_change_signature`. If the specialized tool reports unsupported or blocked, fallback to manual edits and inspect diffs carefully.\n"
            ));
        }
    }

    unindexed_callers.retain(|p| {
        let norm = std::fs::canonicalize(p).unwrap_or_else(|_| p.clone());
        !seen_files.contains(&norm)
    });
    unindexed_callers.sort();
    unindexed_callers.dedup();
    if !unindexed_callers.is_empty() {
        let list = unindexed_callers
            .iter()
            .map(|p| {
                p.strip_prefix(workspace_root)
                    .unwrap_or(p)
                    .display()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!(
            "\n(warning: references may have incomplete coverage across configured targets: unindexed call site(s) found in {list})"
        ));
    }

    Ok(McpToolCallResult::text(out.trim_end()))
}

/// `code_references` in this checkout and in each directory of `also_in`, the name resolved in
/// each, every answer under its checkout's directory; a checkout that fails says why without
/// hiding the others (#375).
pub(crate) async fn references_across(
    remote: SocketAddr,
    workspace_root: &Path,
    args: serde_json::Value,
    dirs: &[serde_json::Value],
) -> Result<McpToolCallResult> {
    if args
        .get("symbol")
        .and_then(|v| v.as_str())
        .is_none_or(|s| s.trim().is_empty())
    {
        anyhow::bail!(
            "`also_in` goes with `symbol`: a position is a place in one checkout, a name is \
             resolved in each"
        );
    }
    let mut roots = vec![workspace_root.to_path_buf()];
    for dir in dirs {
        let dir = dir
            .as_str()
            .context("`also_in` lists the directories of other checkouts")?;
        let dir = resolve_file_path(workspace_root, dir);
        let dir = std::fs::canonicalize(&dir).unwrap_or(dir);
        if !roots.contains(&dir) {
            roots.push(dir);
        }
    }
    let mut out = String::new();
    let mut found = 0usize;
    for (index, root) in roots.iter().enumerate() {
        let mut asked = args.clone();
        if let Some(obj) = asked.as_object_mut() {
            obj.remove("also_in");
            // A file hint names a file of the first checkout.
            if index > 0 {
                obj.remove("path");
            }
        }
        // Each checkout is asked on the node its own workspace is placed on.
        let node = if index == 0 || !root.is_dir() {
            Ok(remote)
        } else {
            crate::cluster::route_for_checkout(remote, root).await
        };
        let text = match node {
            _ if !root.is_dir() => "not a directory".to_string(),
            Err(err) => format!("{err:#}"),
            Ok(node) => match Box::pin(execute_tool(node, root, "code_references", asked)).await {
                Ok(result) => result
                    .content
                    .iter()
                    .map(|item| {
                        let crate::protocol::McpContentItem::Text { text } = item;
                        text.as_str()
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
                Err(err) => format!("{err:#}"),
            },
        };
        found += text.lines().filter(|l| l.starts_with("  • ")).count();
        out.push_str(&format!(
            "== {} ==\n{}\n\n",
            root.display(),
            text.trim_end()
        ));
    }
    out.push_str(&format!(
        "{found} reference(s) in {} checkout(s)",
        roots.len()
    ));
    Ok(McpToolCallResult::text(out))
}

/// The text of 1-based `line` of `file`: read here, or from the node for a file only the node
/// has (a dependency's source). `None` when it cannot be read.
pub(crate) async fn position_line(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
) -> Option<String> {
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(_) if crate::remote_fs::is_external(root, &file.to_string_lossy()) => {
            let (bytes, _) = crate::remote_fs::read_remote_file(remote, &file.to_string_lossy(), 0)
                .await
                .ok()?;
            String::from_utf8_lossy(&bytes).into_owned()
        }
        Err(_) => return None,
    };
    text.lines()
        .nth((line as usize).checked_sub(1)?)
        .map(str::to_string)
}

/// The name a 1-based `character` of a line stands on, or just after (where an editor's cursor
/// sits at the end of a word). `None` for a position on no name.
pub(crate) fn name_at(line: &str, character: u32) -> Option<String> {
    let chars: Vec<char> = line.chars().collect();
    let is_name = |c: &char| c.is_alphanumeric() || *c == '_';
    let at = (character as usize).checked_sub(1)?;
    let at = if chars.get(at).is_some_and(is_name) {
        at
    } else if at > 0 && chars.get(at - 1).is_some_and(is_name) {
        at - 1
    } else {
        return None;
    };
    let start = chars[..at]
        .iter()
        .rposition(|c| !is_name(c))
        .map_or(0, |i| i + 1);
    let end = chars[at..]
        .iter()
        .position(|c| !is_name(c))
        .map_or(chars.len(), |i| at + i);
    Some(chars[start..end].iter().collect())
}

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
