/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Outline extraction, filtering and rendering.

use std::net::SocketAddr;
use std::path::Path;

use anyhow::{Context, Result};

use crate::protocol::McpToolCallResult;
use crate::tools::resolve_file_path;

pub mod directory;
pub mod file;
pub mod markdown;
pub mod options;
pub mod protobuf;
pub mod render;

pub use directory::outline_directory;
pub use file::outline_file;
pub use options::{DIRECTORY_OUTLINE_BYTES, OutlineOptions};
pub use protobuf::protobuf_outline;
pub use render::{render_outline, render_outline_with};

pub async fn handle_outline(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let file_path = resolve_file_path(workspace_root, path_str);
    let is_dir = file_path.is_dir();
    let limit = |key: &str| args.get(key).and_then(|v| v.as_u64()).map(|n| n as usize);
    let options = OutlineOptions {
        max_depth: args
            .get("max_depth")
            .and_then(|v| v.as_u64())
            .unwrap_or(3)
            .max(1) as usize,
        include_locals: args
            .get("include_locals")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        hint: "pass include_locals: true".to_string(),
        kinds: args.get("kinds").and_then(|v| v.as_array()).map(|kinds| {
            kinds
                .iter()
                .filter_map(|k| k.as_str().map(str::to_string))
                .collect()
        }),
        exported_only: args
            .get("exported_only")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        max_bytes: match limit("max_bytes") {
            Some(0) => None,
            Some(bytes) => Some(bytes),
            None => is_dir.then_some(DIRECTORY_OUTLINE_BYTES),
        },
        max_items: limit("max_items").filter(|n| *n > 0),
    };
    let text = if is_dir {
        outline_directory(
            remote,
            workspace_root,
            &file_path,
            Path::new(path_str),
            &options,
        )
        .await?
    } else {
        outline_file(remote, workspace_root, &file_path, path_str, &options).await?
    };
    let has_symbols = text.lines().any(|l| l.trim_start().starts_with('['))
        || (is_dir && text.contains("subdirectories with sources:"));
    if !has_symbols {
        return Ok(McpToolCallResult::error(format!(
            "no outline symbols found for {path_str}"
        )));
    }
    Ok(McpToolCallResult::text(text))
}
