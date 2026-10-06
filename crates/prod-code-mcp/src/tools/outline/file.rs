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

use super::directory::cut_block;
use super::markdown::markdown_outline;
use super::options::OutlineOptions;
use super::protobuf::protobuf_outline;
use super::render::render_outline_with;
use crate::tools::execute_lsp_query;

/// A file's outline, for the MCP tool and the CLI alike (#362): a Markdown file's headings,
/// read here since no language server serves Markdown; an error for a file no language server
/// serves, rather than the empty answer the checkout's server gives for it; otherwise the file's
/// server's `textDocument/documentSymbol` answer. `hint` says how to list the locals.
pub async fn outline_file(
    remote: SocketAddr,
    workspace_root: &Path,
    file_path: &Path,
    path_str: &str,
    options: &OutlineOptions,
) -> Result<String> {
    let extension = file_path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    if matches!(extension.as_deref(), Some("md" | "markdown")) {
        let text = std::fs::read_to_string(file_path)
            .with_context(|| format!("reading {}", file_path.display()))?;
        let rendered = markdown_outline(&text, path_str, options);
        let (text, _) = cut_block(
            &rendered,
            options.max_bytes.unwrap_or(usize::MAX),
            options.max_items.unwrap_or(usize::MAX),
        );
        return Ok(text);
    }
    if matches!(extension.as_deref(), Some("proto")) {
        let text = std::fs::read_to_string(file_path)
            .with_context(|| format!("reading {}", file_path.display()))?;
        let rendered = protobuf_outline(&text, path_str, options);
        let (text, _) = cut_block(
            &rendered,
            options.max_bytes.unwrap_or(usize::MAX),
            options.max_items.unwrap_or(usize::MAX),
        );
        return Ok(text);
    }
    let engine = crate::sync::engine_for_file(file_path);
    if !matches!(
        engine,
        Some("rust" | "go" | "python" | "typescript" | "cpp" | "swift")
    ) {
        let kind = extension.map_or_else(
            || "files without an extension".to_string(),
            |e| format!("`.{e}` files"),
        );
        anyhow::bail!(
            "no outline for {path_str}: language not supported (no language server serves {kind}). \
             prod-code serves Rust, Go, C, C++ and Objective-C, TypeScript and JavaScript, Python \
             and Swift, and outlines Markdown by its headings and Protobuf declarations"
        );
    }
    let file_uri = Url::from_file_path(file_path)
        .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {:?}", file_path))?
        .to_string();
    let params = serde_json::json!({
        "textDocument": { "uri": file_uri }
    });
    let res = execute_lsp_query(
        remote,
        workspace_root,
        file_path,
        "textDocument/documentSymbol",
        params,
    )
    .await?;
    let source = std::fs::read_to_string(file_path).ok();
    let (text, _) = render_outline_with(&res, path_str, options, source.as_deref());
    let (text, _) = cut_block(
        &text,
        options.max_bytes.unwrap_or(usize::MAX),
        options.max_items.unwrap_or(usize::MAX),
    );
    Ok(text)
}
