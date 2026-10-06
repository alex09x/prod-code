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

use anyhow::{Context, Result};
use url::Url;

use crate::protocol::McpToolCallResult;
use crate::tools::{checked_position_argument, execute_lsp_query, resolve_file_path};

pub(crate) async fn handle_rename(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let line =
        checked_position_argument(args.get("line").context("Missing 'line' argument")?, "line")?;
    let character = checked_position_argument(
        args.get("character")
            .context("Missing 'character' argument")?,
        "character",
    )?;
    let new_name = args
        .get("new_name")
        .and_then(|v| v.as_str())
        .context("Missing 'new_name' argument")?
        .to_string();
    let file_path = resolve_file_path(workspace_root, path_str);
    if args
        .get("accessors")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
        return rename_with_accessors(
            remote,
            workspace_root,
            &file_path,
            line,
            character,
            &new_name,
            force,
        )
        .await;
    }
    let file_uri = Url::from_file_path(&file_path)
        .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {:?}", file_path))?
        .to_string();
    let params = serde_json::json!({
        "textDocument": { "uri": file_uri },
        "position": { "line": line.saturating_sub(1), "character": character.saturating_sub(1) },
        "newName": new_name
    });
    let edit = match execute_lsp_query(
        remote,
        workspace_root,
        &file_path,
        "textDocument/rename",
        params,
    )
    .await
    {
        Ok(edit) => edit,
        Err(e) => return Ok(McpToolCallResult::error(format!("rename refused: {e:#}"))),
    };
    if edit.is_null() {
        return Ok(McpToolCallResult::error(
            "rename produced no edits".to_string(),
        ));
    }
    // The analyzer computes the edit; it does not check that the result compiles. A new name
    // that is already declared in the same scope is renamed into a second definition (#98), so
    // the result is checked in the overlay like every other write, and refused if it breaks.
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let (mut planned, moves_files) = crate::refactor::planned_texts(workspace_root, &edit)?;
    // The old name in comments and test names, in every file the rename touches.
    let comments = args
        .get("comments")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let mut mentioned = crate::rename_mentions::Mentions::default();
    if comments {
        if moves_files {
            return Ok(McpToolCallResult::error(
                "`comments` is not supported with a rename that moves files; rename first, then \
                 run it again at the new name"
                    .to_string(),
            ));
        }
        let text = std::fs::read_to_string(&file_path).unwrap_or_default();
        // A position on no character names no old name; the file's first word is not one.
        let Some(at) = crate::signature::offset_of(&text, line, character) else {
            return Ok(McpToolCallResult::error(format!(
                "{path_str}:{line}:{character} is not a position in the file, so the old name \
                 in comments cannot be found; nothing was written"
            )));
        };
        let start = text[..at]
            .rfind(|c: char| !(c.is_alphanumeric() || c == '_'))
            .map_or(0, |i| i + 1);
        let old: String = text[start..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !planned.iter().any(|(p, _)| *p == file_path) {
            planned.push((file_path.clone(), text.clone()));
        }
        for (_, t) in planned.iter_mut() {
            let (rewritten, found) = crate::rename_mentions::rewrite(t, &old, &new_name);
            mentioned.comments += found.comments;
            mentioned.tests.extend(found.tests);
            *t = rewritten;
        }
    }
    let reports = crate::diagnostics::validate_texts(remote, workspace_root, &planned, &[]).await?;
    let errors: Vec<String> = reports
        .iter()
        .flat_map(|r| {
            r.items
                .iter()
                .filter(|d| d.severity == "error")
                .map(move |d| {
                    format!(
                        "{}{} ({}:{}:{})",
                        d.message.lines().next().unwrap_or(""),
                        d.code
                            .as_deref()
                            .map(|c| format!(" [{c}]"))
                            .unwrap_or_default(),
                        r.file,
                        d.line,
                        d.col
                    )
                })
        })
        .collect();
    if !errors.is_empty() && !force {
        return Ok(McpToolCallResult::error(format!(
            "rename to `{new_name}` refused: the result does not compile ({} error(s)); nothing \
             was written. If `{new_name}` is already declared in that scope, pick another name; \
             pass `force: true` to write it anyway:\n  {}",
            errors.len(),
            errors.join("\n  ")
        )));
    }
    // With `comments` the texts are no longer the analyzer's edit alone: write them whole.
    let touched = if comments {
        let files: std::collections::BTreeMap<std::path::PathBuf, String> = planned
            .into_iter()
            .filter(|(p, t)| std::fs::read_to_string(p).map(|o| o != *t).unwrap_or(true))
            .collect();
        crate::refactor::apply_workspace_edit(
            workspace_root,
            &crate::signature::whole_file_edit(&files),
        )?
    } else {
        crate::refactor::apply_workspace_edit(workspace_root, &edit)?
    };
    let mut text = format!(
        "renamed to `{new_name}`; {} path(s) updated in the checkout:\n{}",
        touched.len(),
        touched.join("\n")
    );
    if comments {
        text.push_str(&format!(
            "\n\nin comments: {} mention(s) of the old name replaced",
            mentioned.comments
        ));
        for (from, to) in &mentioned.tests {
            text.push_str(&format!("\ntest renamed: `{from}` -> `{to}`"));
        }
    }
    if !errors.is_empty() {
        text.push_str(&format!(
            "\n\nwritten with `force`, although the analyzer reports {} error(s):\n  {}",
            errors.len(),
            errors.join("\n  ")
        ));
    }
    if moves_files {
        text.push_str("\n\nthe rename also moved files; that part was not checked before writing");
    }
    Ok(McpToolCallResult::text(text))
}

/// A field renamed together with its accessors (#146): every rename merged into one change per
/// file, checked in one overlay, written only when it compiles unless `force`.
async fn rename_with_accessors(
    remote: SocketAddr,
    workspace_root: &Path,
    file: &Path,
    line: u32,
    character: u32,
    new_name: &str,
    force: bool,
) -> Result<McpToolCallResult> {
    let (merged, renamed) = match crate::rename_accessors::plan(
        remote,
        workspace_root,
        file,
        line,
        character,
        new_name,
    )
    .await
    {
        Ok(plan) => plan,
        Err(e) => return Ok(McpToolCallResult::error(format!("rename refused: {e:#}"))),
    };
    if merged.is_empty() {
        return Ok(McpToolCallResult::error(
            "rename produced no edits".to_string(),
        ));
    }
    let planned: Vec<(std::path::PathBuf, String)> =
        merged.iter().map(|(p, t)| (p.clone(), t.clone())).collect();
    let reports = crate::diagnostics::validate_texts(remote, workspace_root, &planned, &[]).await?;
    let errors: Vec<String> = reports
        .iter()
        .flat_map(|r| {
            r.items
                .iter()
                .filter(|d| d.severity == "error")
                .map(move |d| {
                    format!(
                        "{}{} ({}:{}:{})",
                        d.message.lines().next().unwrap_or(""),
                        d.code
                            .as_deref()
                            .map(|c| format!(" [{c}]"))
                            .unwrap_or_default(),
                        r.file,
                        d.line,
                        d.col
                    )
                })
        })
        .collect();
    if !errors.is_empty() && !force {
        return Ok(McpToolCallResult::error(format!(
            "rename refused: the result does not compile ({} error(s)); nothing was written:\n  {}",
            errors.len(),
            errors.join("\n  ")
        )));
    }
    let touched = crate::refactor::apply_workspace_edit(
        workspace_root,
        &crate::signature::whole_file_edit(&merged),
    )?;
    Ok(McpToolCallResult::text(format!(
        "renamed {}; {} path(s) updated in the checkout:\n{}",
        renamed.join(", "),
        touched.len(),
        touched.join("\n")
    )))
}
