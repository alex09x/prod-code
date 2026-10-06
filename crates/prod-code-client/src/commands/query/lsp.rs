/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::commands::common::print_locations;
use crate::commands::query::lsp_client::execute_lsp_query;
use crate::workspace::find_workspace_root;
use anyhow::Result;
use std::net::SocketAddr;
use std::path::Path;
use url::Url;

pub async fn run_hover(remote: SocketAddr, file: &Path, line: u32, col: u32) -> Result<()> {
    let abs_path = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let file_uri = Url::from_file_path(&abs_path)
        .map_err(|_| anyhow::anyhow!("Invalid file path"))?
        .to_string();

    let lsp_line = line.saturating_sub(1);
    let lsp_col = col.saturating_sub(1);

    let params = serde_json::json!({
        "textDocument": { "uri": file_uri },
        "position": { "line": lsp_line, "character": lsp_col }
    });

    let result = execute_lsp_query(remote, file, "textDocument/hover", params).await?;

    if let Some(contents) = result.get("contents") {
        if let Some(value) = contents.get("value").and_then(|v| v.as_str()) {
            if !value.trim().is_empty() {
                println!("{value}");
                return Ok(());
            }
        } else if let Some(arr) = contents.as_array() {
            let mut found = false;
            for item in arr {
                let v = item
                    .get("value")
                    .and_then(|v| v.as_str())
                    .or_else(|| item.as_str());
                if let Some(v) = v {
                    if !v.trim().is_empty() {
                        println!("{v}");
                        found = true;
                    }
                }
            }
            if found {
                return Ok(());
            }
        } else if let Some(s) = contents.as_str() {
            if !s.trim().is_empty() {
                println!("{s}");
                return Ok(());
            }
        }
    }

    anyhow::bail!(
        "no hover information found at {}:{}:{}",
        file.display(),
        line,
        col
    );
}

pub async fn run_definition(remote: SocketAddr, file: &Path, line: u32, col: u32) -> Result<()> {
    let abs_path = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let file_uri = Url::from_file_path(&abs_path)
        .map_err(|_| anyhow::anyhow!("Invalid file path"))?
        .to_string();

    let lsp_line = line.saturating_sub(1);
    let lsp_col = col.saturating_sub(1);

    let params = serde_json::json!({
        "textDocument": { "uri": file_uri },
        "position": { "line": lsp_line, "character": lsp_col }
    });

    let result = execute_lsp_query(remote, file, "textDocument/definition", params).await?;
    let ws_root = find_workspace_root(&abs_path).unwrap_or_else(|| abs_path.clone());
    let mut shown = 0;

    if let Some(arr) = result.as_array() {
        if arr.is_empty() {
            anyhow::bail!("no definition found at {}:{}:{}", file.display(), line, col);
        } else {
            for loc in arr {
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
                println!("📍 Definition: {uri}:{start_line}:{start_col}");
                let path = prod_code_mcp::remote_fs::uri_to_path(uri);
                if prod_code_mcp::remote_fs::is_external(&ws_root, &path) && shown < 3 {
                    shown += 1;
                    match prod_code_mcp::remote_fs::read_remote_file(remote, &path, 0).await {
                        Ok((bytes, _)) => {
                            let text = String::from_utf8_lossy(&bytes);
                            print!(
                                "{}",
                                prod_code_mcp::remote_fs::snippet(&text, start_line as u32, 8)
                            );
                        }
                        Err(e) => println!("   (external source not readable: {e})"),
                    }
                }
            }
            return Ok(());
        }
    } else if let Some(obj) = result.as_object() {
        if !obj.is_empty() {
            let uri = obj.get("uri").and_then(|u| u.as_str()).unwrap_or("");
            let start_line = obj
                .get("range")
                .and_then(|r| r.get("start"))
                .and_then(|s| s.get("line"))
                .and_then(|l| l.as_u64())
                .unwrap_or(0)
                + 1;
            let start_col = obj
                .get("range")
                .and_then(|r| r.get("start"))
                .and_then(|s| s.get("character"))
                .and_then(|c| c.as_u64())
                .unwrap_or(0)
                + 1;
            println!("📍 Definition: {uri}:{start_line}:{start_col}");
            return Ok(());
        }
    }

    anyhow::bail!("no definition found at {}:{}:{}", file.display(), line, col);
}

pub async fn run_implementations(remote: SocketAddr, file: &Path, line: u32, col: u32) -> Result<()> {
    let abs_path = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let file_uri = Url::from_file_path(&abs_path)
        .map_err(|_| anyhow::anyhow!("Invalid file path"))?
        .to_string();
    let params = serde_json::json!({
        "textDocument": { "uri": file_uri },
        "position": { "line": line.saturating_sub(1), "character": col.saturating_sub(1) },
    });
    let result = execute_lsp_query(remote, file, "textDocument/implementation", params).await?;
    let arr = match &result {
        serde_json::Value::Array(a) => a.clone(),
        serde_json::Value::Object(_) => vec![result.clone()],
        _ => Vec::new(),
    };
    if arr.is_empty() {
        println!("No implementations found.");
    } else {
        println!("Found {} implementation(s):", arr.len());
        print_locations(&arr);
    }
    Ok(())
}
