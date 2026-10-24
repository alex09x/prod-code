/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::nested_projects::representative_source_file;
use anyhow::{Context, Result};
use std::net::SocketAddr;
use std::path::Path;

pub(super) async fn absolute_symbol(
    remote: SocketAddr,
    root: &Path,
    symbol: &str,
    hint: Option<&Path>,
) -> Result<String> {
    if !symbol.starts_with("self::") && !symbol.starts_with("super::") {
        return Ok(symbol.to_string());
    }
    let anchor = hint
        .map(|p| {
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                root.join(p)
            }
        })
        .filter(|p| p.is_file())
        .or_else(|| representative_source_file(root))
        .context("relative Rust symbol needs a source file in `path`")?;
    let uri = url::Url::from_file_path(&anchor)
        .map_err(|_| anyhow::anyhow!("invalid relative symbol source path"))?;
    let response = crate::tools::execute_lsp_query(
        remote,
        root,
        &anchor,
        "prodCode/modulePath",
        serde_json::json!({ "textDocument": { "uri": uri.to_string() } }),
    )
    .await?;
    let module = response.as_str().context("the analyzer could not establish the source file's module; supply a fully qualified symbol")?;
    let mut path: Vec<&str> = module.split("::").collect();
    let mut suffix = symbol;
    if let Some(rest) = suffix.strip_prefix("self::") {
        suffix = rest;
    }
    while let Some(rest) = suffix.strip_prefix("super::") {
        anyhow::ensure!(
            path.len() > 1,
            "no symbol named `{symbol}`: `super` goes above the crate root"
        );
        path.pop();
        suffix = rest;
    }
    Ok(format!("{}::{suffix}", path.join("::")))
}
