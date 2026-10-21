/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::matching::{bare_symbol_name, match_rank};
use super::types::{MalformedLspCoordinate, SymbolHit, lsp_position, symbol_kind_name};
use anyhow::Result;
use std::path::Path;

/// Rust structure labels describe a binding pattern (`ref mut items`), while callers name
/// its identifier. Other languages and non-variable declarations keep their own labels.
pub(crate) fn binding_name<'a>(file: &Path, kind: u64, label: &'a str) -> &'a str {
    if kind != 13 || file.extension().and_then(|e| e.to_str()) != Some("rs") {
        return label;
    }
    let strip_mode = |name: &'a str, mode: &str| {
        name.strip_prefix(mode)
            .filter(|rest| rest.starts_with(char::is_whitespace))
            .map(str::trim_start)
            .unwrap_or(name)
    };
    let name = strip_mode(strip_mode(label, "ref"), "mut");
    if !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '#')
    {
        name
    } else {
        label
    }
}

/// Adjust a structure selection that starts at the binding mode to the identifier. Some
/// servers already select only the identifier; readable local source distinguishes those.
pub(crate) fn binding_column(file: &Path, label: &str, name: &str, line: u32, col: u32) -> u32 {
    if label == name {
        return col;
    }
    if let Ok(source) = std::fs::read_to_string(file)
        && let Some(row) = source.lines().nth(line.saturating_sub(1) as usize)
    {
        let at: String = row.chars().skip(col.saturating_sub(1) as usize).collect();
        if at.starts_with(name) {
            return col;
        }
    }
    col.saturating_add(label[..label.len() - name.len()].chars().count() as u32)
}

/// Walks a document outline for declarations, converting Rust binding patterns to names.
pub(crate) fn collect_named(
    symbols: &serde_json::Value,
    name: &str,
    parent: Option<&str>,
    file: &Path,
    out: &mut Vec<SymbolHit>,
) -> Result<()> {
    for symbol in symbols.as_array().into_iter().flatten() {
        let label = symbol.get("name").and_then(|n| n.as_str()).unwrap_or("");
        let kind = symbol.get("kind").and_then(|k| k.as_u64()).unwrap_or(0);
        let own = binding_name(file, kind, label);
        if bare_symbol_name(own).eq_ignore_ascii_case(name) || match_rank(own, name) < 4 {
            let start = symbol
                .pointer("/selectionRange/start")
                .or_else(|| symbol.pointer("/location/range/start"))
                .or_else(|| symbol.pointer("/range/start"));
            if let Some(start) = start {
                let (line, col) = lsp_position(start, &format!("named declaration `{own}`"))?;
                out.push(SymbolHit {
                    path: file.to_path_buf(),
                    name: own.to_string(),
                    kind: symbol_kind_name(kind),
                    container: parent.map(str::to_string).or_else(|| {
                        symbol
                            .get("containerName")
                            .and_then(|c| c.as_str())
                            .filter(|c| !c.is_empty())
                            .map(str::to_string)
                    }),
                    line,
                    col: binding_column(file, label, own, line, col),
                });
            } else if bare_symbol_name(own).eq_ignore_ascii_case(name) {
                return Err(anyhow::Error::new(MalformedLspCoordinate(format!(
                    "malformed LSP named declaration `{own}`: missing selection range start"
                ))));
            }
        }
        if let Some(children) = symbol.get("children") {
            collect_named(children, name, Some(own), file, out)?;
        }
    }
    Ok(())
}
