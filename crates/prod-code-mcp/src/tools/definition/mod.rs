/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Definition retrieval, body extraction, doc comment recovery, and line numbering.

use std::net::SocketAddr;
use std::path::Path;

use anyhow::{Context, Result};
use url::Url;

use super::{execute_lsp_query, resolve_file_path};
use crate::protocol::McpToolCallResult;

pub(crate) mod body;
pub(crate) mod range;

pub(crate) use body::*;
pub(crate) use range::*;

pub(crate) async fn handle_definition(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .or_else(|| args.get("file_path"))
        .or_else(|| args.get("file"))
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
    let body = args.get("body").and_then(|v| v.as_bool()).unwrap_or(true);
    let file_path = resolve_file_path(workspace_root, path_str);
    let file_uri = Url::from_file_path(&file_path)
        .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {:?}", file_path))?
        .to_string();
    let params = serde_json::json!({
        "textDocument": { "uri": file_uri },
        "position": { "line": line.saturating_sub(1), "character": character.saturating_sub(1) }
    });
    let res = execute_lsp_query(
        remote,
        workspace_root,
        &file_path,
        "textDocument/definition",
        params,
    )
    .await?;
    let mut out = String::new();
    let has_locations = match &res {
        serde_json::Value::Array(arr) => !arr.is_empty(),
        serde_json::Value::Object(_) => true,
        _ => false,
    };
    if has_locations {
        if let Some(arr) = res.as_array() {
            for (i, loc) in arr.iter().enumerate() {
                let uri = loc
                    .get("uri")
                    .or_else(|| loc.get("targetUri"))
                    .and_then(|u| u.as_str())
                    .unwrap_or("");
                let range = loc.get("range").or_else(|| loc.get("targetSelectionRange"));
                let start_line = range
                    .and_then(|r| r.get("start"))
                    .and_then(|s| s.get("line"))
                    .and_then(|l| l.as_u64())
                    .unwrap_or(0)
                    + 1;
                let start_col = range
                    .and_then(|r| r.get("start"))
                    .and_then(|s| s.get("character"))
                    .and_then(|c| c.as_u64())
                    .unwrap_or(0)
                    + 1;
                if i > 0 {
                    out.push('\n');
                }
                out.push_str(&format!("📍 Definition: {uri}:{start_line}:{start_col}"));
                if body && i < 3 {
                    match definition_body(
                        remote,
                        workspace_root,
                        uri,
                        start_line as u32,
                        start_col as u32,
                    )
                    .await
                    {
                        Ok(text) => {
                            out.push('\n');
                            out.push_str(&text);
                        }
                        Err(e) => out.push_str(&format!(
                            "\n   (the definition's code could not be read: {e:#})"
                        )),
                    }
                    continue;
                }
                // Outside the checkout the file exists only on the gateway: include
                // the lines around the definition so the agent can read it.
                let path = crate::remote_fs::uri_to_path(uri);
                if i < 3 && crate::remote_fs::is_external(workspace_root, &path) {
                    match crate::remote_fs::read_remote_file(remote, &path, 0).await {
                        Ok((bytes, _)) => {
                            let text = String::from_utf8_lossy(&bytes);
                            out.push('\n');
                            out.push_str(&crate::remote_fs::snippet(&text, start_line as u32, 8));
                        }
                        Err(e) => {
                            out.push_str(&format!("\n   (external source not readable: {e})"))
                        }
                    }
                }
            }
        } else if let Some(obj) = res.as_object() {
            let uri = obj
                .get("uri")
                .or_else(|| obj.get("targetUri"))
                .and_then(|u| u.as_str())
                .unwrap_or("");
            let range = obj.get("range").or_else(|| obj.get("targetSelectionRange"));
            let start_line = range
                .and_then(|r| r.get("start"))
                .and_then(|s| s.get("line"))
                .and_then(|l| l.as_u64())
                .unwrap_or(0)
                + 1;
            let start_col = range
                .and_then(|r| r.get("start"))
                .and_then(|s| s.get("character"))
                .and_then(|c| c.as_u64())
                .unwrap_or(0)
                + 1;
            out.push_str(&format!("📍 Definition: {uri}:{start_line}:{start_col}"));
            if body {
                match definition_body(
                    remote,
                    workspace_root,
                    uri,
                    start_line as u32,
                    start_col as u32,
                )
                .await
                {
                    Ok(text) => {
                        out.push('\n');
                        out.push_str(&text);
                    }
                    Err(e) => out.push_str(&format!(
                        "\n   (the definition's code could not be read: {e:#})"
                    )),
                }
            } else {
                let path = crate::remote_fs::uri_to_path(uri);
                if crate::remote_fs::is_external(workspace_root, &path) {
                    match crate::remote_fs::read_remote_file(remote, &path, 0).await {
                        Ok((bytes, _)) => {
                            let text = String::from_utf8_lossy(&bytes);
                            out.push('\n');
                            out.push_str(&crate::remote_fs::snippet(&text, start_line as u32, 8));
                        }
                        Err(e) => {
                            out.push_str(&format!("\n   (external source not readable: {e})"))
                        }
                    }
                }
            }
        }
    } else {
        let is_def = if args.get("symbol").is_some() {
            true
        } else {
            is_outline_definition(remote, workspace_root, &file_path, line, character).await
        };
        if is_def {
            out.push_str(&format!("📍 Definition: {file_uri}:{line}:{character}"));
            if body {
                match definition_body(remote, workspace_root, &file_uri, line, character).await {
                    Ok(text) => {
                        out.push('\n');
                        out.push_str(&text);
                    }
                    Err(e) => out.push_str(&format!(
                        "\n   (the definition's code could not be read: {e:#})"
                    )),
                }
            }
        } else {
            out.push_str("No definition found.");
        }
    }
    Ok(McpToolCallResult::text(out))
}
