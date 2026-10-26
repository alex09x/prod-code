/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::Result;
use std::{net::SocketAddr, path::Path};

use crate::signature::references::{parse_locations, references};
use crate::signature::types::Reference;

/// Reconcile C/C++ redeclarations with the same analyzer identity. A matching name,
/// namespace spelling or parameter list is never evidence of identity.
pub async fn references_and_declarations(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    cpp: bool,
) -> Result<Vec<Reference>> {
    let mut found = references(remote, root, file, line, col).await?;
    if !cpp {
        return Ok(found);
    }
    let params = serde_json::json!({
        "textDocument": {"uri": url::Url::from_file_path(file).unwrap().to_string()},
        "position": {"line": line.saturating_sub(1), "character": col.saturating_sub(1)},
        "context": {"includeDeclaration": true},
    });
    for method in [
        "textDocument/references",
        "textDocument/declaration",
        "textDocument/definition",
    ] {
        let value =
            crate::tools::execute_lsp_query(remote, root, file, method, params.clone()).await?;
        let locations = if value.is_null() {
            Vec::new()
        } else if let Some(array) = value.as_array() {
            array.clone()
        } else {
            vec![value]
        };
        let locations = locations.into_iter().map(|location| {
            if location.get("targetUri").is_some() {
                serde_json::json!({"uri": location["targetUri"], "range": location["targetSelectionRange"]})
            } else {
                location
            }
        }).collect::<Vec<_>>();
        found.extend(parse_locations(&serde_json::json!(locations))?);
    }
    found.sort();
    found.dedup();
    Ok(found)
}
