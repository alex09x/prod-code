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

/// The type the analyzer's hover gives a local: `bool` for `let flag: bool`.
pub(crate) async fn local_type(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
) -> Option<String> {
    let uri = url::Url::from_file_path(file).ok()?.to_string();
    let res = crate::tools::execute_lsp_query(
        remote,
        root,
        file,
        "textDocument/hover",
        serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": line.saturating_sub(1), "character": col.saturating_sub(1) },
        }),
    )
    .await
    .ok()?;
    let value = res.pointer("/contents/value").and_then(|v| v.as_str())?;
    hover_type(value)
}

/// `bool` out of a hover like "```rust\nlet flag: bool\n```".
pub fn hover_type(hover: &str) -> Option<String> {
    let line = hover.lines().find(|l| l.trim_start().starts_with("let "))?;
    let (_, ty) = line.split_once(':')?;
    Some(ty.trim().to_string())
}

pub(crate) fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}
