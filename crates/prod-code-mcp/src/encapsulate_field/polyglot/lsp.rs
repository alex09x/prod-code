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

use anyhow::Result;

pub fn lsp_symbol_position(symbol: &serde_json::Value) -> Option<(u32, u32)> {
    let start = symbol
        .pointer("/selectionRange/start")
        .or_else(|| symbol.pointer("/location/range/start"))
        .or_else(|| symbol.pointer("/range/start"))?;
    let line = u32::try_from(start.get("line")?.as_u64()?)
        .ok()?
        .checked_add(1)?;
    let col = u32::try_from(start.get("character")?.as_u64()?)
        .ok()?
        .checked_add(1)?;
    Some((line, col))
}

pub fn field_position_in_symbols(
    symbols: &serde_json::Value,
    owner: &str,
    field: &str,
) -> Option<(u32, u32)> {
    fn visit(
        symbols: &[serde_json::Value],
        owner: &str,
        field: &str,
        inside_owner: bool,
    ) -> Option<(u32, u32)> {
        for symbol in symbols {
            let name = symbol.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let is_owner = name == owner;
            if (inside_owner || is_owner) && name.trim_start_matches('#') == field {
                if let Some(position) = lsp_symbol_position(symbol) {
                    return Some(position);
                }
            }
            if name.trim_start_matches('#') == field
                && symbol.get("containerName").and_then(|n| n.as_str()) == Some(owner)
                && let Some(position) = lsp_symbol_position(symbol)
            {
                return Some(position);
            }
            if let Some(children) = symbol.get("children").and_then(|v| v.as_array())
                && let Some(position) = visit(children, owner, field, inside_owner || is_owner)
            {
                return Some(position);
            }
        }
        None
    }

    visit(symbols.as_array()?, owner, field, false)
}

pub(crate) async fn field_position_in_lsp(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    owner: &str,
    field: &str,
) -> Result<Option<(u32, u32)>> {
    let uri = url::Url::from_file_path(file)
        .map_err(|_| anyhow::anyhow!("invalid field source path {:?}", file))?
        .to_string();
    let symbols = crate::tools::execute_lsp_query(
        remote,
        root,
        file,
        "textDocument/documentSymbol",
        serde_json::json!({ "textDocument": { "uri": uri } }),
    )
    .await?;
    Ok(field_position_in_symbols(&symbols, owner, field))
}
