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
        let text = std::fs::read_to_string(file_path).with_context(|| {
            crate::remote_fs::missing_file_context(workspace_root, file_path, "reading")
        })?;
        let rendered = markdown_outline(&text, path_str, options);
        let (text, _) = cut_block(
            &rendered,
            options.max_bytes.unwrap_or(usize::MAX),
            options.max_items.unwrap_or(usize::MAX),
        );
        return Ok(text);
    }
    if matches!(extension.as_deref(), Some("proto")) {
        let text = std::fs::read_to_string(file_path).with_context(|| {
            crate::remote_fs::missing_file_context(workspace_root, file_path, "reading")
        })?;
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
    let source = std::fs::read_to_string(file_path).ok();
    let query_fut = execute_lsp_query(
        remote,
        workspace_root,
        file_path,
        "textDocument/documentSymbol",
        params,
    );
    let (res, warmup_note) = if engine == Some("swift") {
        match tokio::time::timeout(std::time::Duration::from_secs(4), query_fut).await {
            Ok(res) => (res?, false),
            Err(_) => (
                local_swift_outline_symbols(source.as_deref().unwrap_or("")),
                true,
            ),
        }
    } else {
        (query_fut.await?, false)
    };
    let (mut text, _) = render_outline_with(&res, path_str, options, source.as_deref());
    if warmup_note {
        text.push_str(
            "\n\n(note: outlined from Swift source while sourcekit-lsp finishes package warmup)",
        );
    }
    let (text, _) = cut_block(
        &text,
        options.max_bytes.unwrap_or(usize::MAX),
        options.max_items.unwrap_or(usize::MAX),
    );
    Ok(text)
}

pub(crate) fn local_swift_outline_symbols(source: &str) -> serde_json::Value {
    let code_mask =
        crate::tools::references::mask_executable_code(source, std::path::Path::new("file.swift"));
    let masked = crate::tools::references::masked_code_string(source, &code_mask);
    let mut items = Vec::new();
    for (idx, line) in masked.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        for (kw, kind) in [
            ("struct ", 23u64),
            ("class ", 5),
            ("enum ", 10),
            ("protocol ", 11),
            ("actor ", 5),
            ("extension ", 5),
            ("func ", 12),
            ("init(", 9),
        ] {
            if let Some(pos) = trimmed.find(kw)
                && (pos == 0 || trimmed[..pos].ends_with(char::is_whitespace))
            {
                let name = if kw == "init(" {
                    "init".to_string()
                } else {
                    trimmed[pos + kw.len()..]
                        .trim()
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '.')
                        .collect()
                };
                if !name.is_empty() {
                    let line_u32 = idx as u32;
                    items.push(serde_json::json!({
                        "name": name,
                        "kind": kind,
                        "range": {
                            "start": { "line": line_u32, "character": 0 },
                            "end": { "line": line_u32, "character": line.len() as u32 }
                        },
                        "selectionRange": {
                            "start": { "line": line_u32, "character": 0 },
                            "end": { "line": line_u32, "character": line.len() as u32 }
                        }
                    }));
                }
                break;
            }
        }
    }
    serde_json::Value::Array(items)
}
