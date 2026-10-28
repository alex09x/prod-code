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
use std::path::Path;

use anyhow::{Context, Result};
use url::Url;

use super::{build_swift_index, execute_lsp_query, resolve_file_path};
use crate::protocol::McpToolCallResult;

pub(crate) mod across;
pub(crate) mod lexical;
mod locations;
pub(crate) mod multi_target;
pub(crate) mod render;
#[cfg(test)]
mod tests;

pub(crate) use across::*;
pub(crate) use locations::add_locations;
pub(crate) use multi_target::*;
use render::render_reference_locations;

pub(crate) async fn handle_references(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let reference_deadline = multi_target_deadline(args, tokio::time::Instant::now());
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
    let context_lines = args
        .get("context")
        .or_else(|| args.get("context_lines"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0)
        .min(50) as usize;
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

    add_locations(
        workspace_root,
        &res,
        &mut seen_locs,
        &mut seen_files,
        &mut all_locs,
    );
    let mut initial_was_empty = all_locs.is_empty();
    let build_index = args
        .get("build_index")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let mut swift_index_attempted = false;
    if initial_was_empty
        && build_index
        && let Some((built, note)) =
            build_swift_index(remote, workspace_root, &file_path, true).await
    {
        swift_index_attempted = true;
        out.push_str(&note);
        out.push('\n');
        if built {
            res = execute_lsp_query(
                remote,
                workspace_root,
                &file_path,
                "textDocument/references",
                params.clone(),
            )
            .await?;
            add_locations(
                workspace_root,
                &res,
                &mut seen_locs,
                &mut seen_files,
                &mut all_locs,
            );
            initial_was_empty = all_locs.is_empty();
        }
    }
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
            reference_deadline,
        )
        .await
    {
        out.push_str(&note);
    }

    // Uses found only by the same-target fallback may be fewer than an index would give, so
    // the hint to build one stands when the server itself found nothing (#1135).
    if (all_locs.is_empty() || (initial_was_empty && !build_index))
        && !swift_index_attempted
        && let Some((built, note)) =
            build_swift_index(remote, workspace_root, &file_path, build_index).await
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
            add_locations(
                workspace_root,
                &res,
                &mut seen_locs,
                &mut seen_files,
                &mut all_locs,
            );
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
        out.push_str(&render_reference_locations(
            workspace_root,
            &all_locs[..display_count],
            context_lines,
        ));
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

pub(crate) fn swift_shares_or_imports_target(
    caller: &Path,
    decl: &Path,
    has_any_import: bool,
) -> bool {
    fn target_dir(p: &Path) -> Option<&Path> {
        p.ancestors().find(|a| {
            a.parent()
                .and_then(|d| d.file_name())
                .and_then(|n| n.to_str())
                .is_some_and(|n| matches!(n, "Sources" | "Tests" | "Plugins"))
        })
    }
    has_any_import
        || caller.parent() == decl.parent()
        || matches!((target_dir(caller), target_dir(decl)), (Some(a), Some(b)) if a == b)
}

pub(crate) fn definition_in_file(found: &serde_json::Value, path: &str) -> bool {
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
        crate::remote_fs::uri_to_path(uri) == path
    })
}

pub(crate) fn definition_in_other_workspace_file(
    found: &serde_json::Value,
    workspace_root: &Path,
    decl_path: &str,
) -> bool {
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
        let resolved = crate::remote_fs::uri_to_path(uri);
        !resolved.is_empty()
            && resolved != decl_path
            && !crate::remote_fs::is_external(workspace_root, &resolved)
    })
}
