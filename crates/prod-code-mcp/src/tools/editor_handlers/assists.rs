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
use crate::tools::{execute_lsp_query, resolve_file_path};

/// rust-analyzer writes a prelude item an assist introduces by its full path — an extracted
/// function returns `std::prelude::v1::Result<T, anyhow::Error>` in a file that imports
/// `anyhow::Result` (#97). On the lines the assist wrote, the path is dropped and the result
/// checked in the overlay; the shorter spelling is used only when the
/// analyzer accepts it, and rust-analyzer's own otherwise. Returns the edit to apply and how
/// many paths were shortened.
async fn prefer_names_in_scope(
    remote: SocketAddr,
    root: &Path,
    edit: serde_json::Value,
) -> Result<(serde_json::Value, usize)> {
    const PRELUDE: &str = "std::prelude::v1::";
    let (planned, moves_files) = crate::refactor::planned_texts(root, &edit)?;
    if moves_files {
        return Ok((edit, 0));
    }
    let mut shortened = 0usize;
    let mut shorter = Vec::with_capacity(planned.len());
    for (path, text) in planned {
        // Only lines the assist wrote: a line that was already in the file keeps its spelling,
        // whatever it says.
        let before = std::fs::read_to_string(&path).unwrap_or_default();
        let old_lines: std::collections::HashSet<&str> = before.lines().collect();
        let mut out = String::with_capacity(text.len());
        for line in text.split_inclusive('\n') {
            let body = line.trim_end_matches('\n');
            if body.contains(PRELUDE) && !old_lines.contains(body) {
                shortened += body.matches(PRELUDE).count();
                out.push_str(&line.replace(PRELUDE, ""));
            } else {
                out.push_str(line);
            }
        }
        shorter.push((path, out));
    }
    if shortened == 0 {
        return Ok((edit, 0));
    }
    let reports = crate::diagnostics::validate_texts(remote, root, &shorter, &[]).await?;
    if reports.iter().any(|r| r.errors > 0) {
        return Ok((edit, 0));
    }
    let files: std::collections::BTreeMap<std::path::PathBuf, String> =
        shorter.into_iter().collect();
    Ok((crate::signature::whole_file_edit(&files), shortened))
}

pub(crate) async fn handle_assists(
    remote: SocketAddr,
    workspace_root: &Path,
    tool_name: &str,
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
    let (end_line, end_char) = match (
        args.get("end_line").and_then(|v| v.as_u64()),
        args.get("end_character").and_then(|v| v.as_u64()),
    ) {
        (Some(l), Some(c)) => (l as u32, c as u32),
        _ => (line, character),
    };
    let file_path = resolve_file_path(workspace_root, path_str);
    let file_uri = Url::from_file_path(&file_path)
        .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {:?}", file_path))?
        .to_string();
    let mut params = serde_json::json!({
        "textDocument": { "uri": file_uri },
        "range": {
            "start": { "line": line.saturating_sub(1), "character": character.saturating_sub(1) },
            "end": { "line": end_line.saturating_sub(1), "character": end_char.saturating_sub(1) }
        }
    });
    if tool_name == "code_assist" {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .context("Missing 'id' argument")?;
        params["id"] = serde_json::json!(id);
        if let Some(subtype) = args.get("subtype").and_then(|v| v.as_u64()) {
            params["subtype"] = serde_json::json!(subtype);
        }
        let edit = match execute_lsp_query(
            remote,
            workspace_root,
            &file_path,
            "prodCode/applyAssist",
            params,
        )
        .await
        {
            Ok(edit) => edit,
            Err(e) => {
                return Ok(McpToolCallResult::error(format!("assist refused: {e:#}")));
            }
        };
        let (edit, respelled) = prefer_names_in_scope(remote, workspace_root, edit).await?;
        let touched = crate::refactor::apply_workspace_edit(workspace_root, &edit)?;
        let mut text = format!(
            "applied `{id}`; {} path(s) updated in the checkout:\n{}",
            touched.len(),
            touched.join("\n")
        );
        if respelled > 0 {
            text.push_str(&format!(
                "\n\n{respelled} `std::prelude::v1::` path(s) the assist wrote are spelled as the \
                 name already in scope; the analyzer accepts the shorter spelling"
            ));
        }
        return Ok(McpToolCallResult::text(text));
    }
    let list = execute_lsp_query(
        remote,
        workspace_root,
        &file_path,
        "prodCode/assists",
        params,
    )
    .await?;
    let mut out = String::new();
    if let Some(items) = list.as_array() {
        if items.is_empty() {
            out.push_str("no code actions at this position\n");
        }
        for item in items {
            let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("?");
            let kind = item.get("kind").and_then(|v| v.as_str()).unwrap_or("");
            let label = item.get("label").and_then(|v| v.as_str()).unwrap_or("");
            match item.get("subtype").and_then(|v| v.as_u64()) {
                Some(st) => out.push_str(&format!("{id} (subtype {st}) [{kind}]: {label}\n")),
                None => out.push_str(&format!("{id} [{kind}]: {label}\n")),
            }
        }
    }
    Ok(McpToolCallResult::text(out.trim_end().to_string()))
}
