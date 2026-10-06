/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::rust::rust_supertypes;
use super::types::{Kind, MAX_DEPTH, Supertype, Supertypes};
use super::validate::{hierarchy_start, validate_type_hierarchy_item};
use crate::tools::execute_lsp_query;
use anyhow::Result;
use std::collections::HashSet;
use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::pin::Pin;

/// The supertypes of the type or trait at the 1-based `line`:`character` of `file`.
pub async fn supertypes(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    character: u32,
    depth: usize,
) -> Result<Supertypes> {
    let depth = depth.clamp(1, MAX_DEPTH);
    let uri = url::Url::from_file_path(file)
        .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {file:?}"))?
        .to_string();
    let position = serde_json::json!({
        "textDocument": { "uri": uri },
        "position": { "line": line.saturating_sub(1), "character": character.saturating_sub(1) },
    });
    if file.extension().is_some_and(|e| e == "rs") {
        return rust_supertypes(remote, root, file, line, character, position, depth).await;
    }
    let prepared = execute_lsp_query(
        remote,
        root,
        file,
        "textDocument/prepareTypeHierarchy",
        position,
    )
    .await?;
    let prepared_array = match &prepared {
        serde_json::Value::Null => {
            return Ok(Supertypes {
                of: format!("{}:{line}:{character}", file.display()),
                kind: Kind::Other,
                list: Vec::new(),
                depth,
                unsupported: Some(format!(
                    "No type hierarchy at {}:{line}:{character}: the language server answered none (not every server has one).",
                    file.display()
                )),
            });
        }
        serde_json::Value::Array(a) => a,
        other => anyhow::bail!(
            "the analyzer's prepareTypeHierarchy answer is not an array or null: {other}"
        ),
    };
    if prepared_array.is_empty() {
        return Ok(Supertypes {
            of: format!("{}:{line}:{character}", file.display()),
            kind: Kind::Other,
            list: Vec::new(),
            depth,
            unsupported: Some(format!(
                "No type hierarchy at {}:{line}:{character}: the language server answered none (not every server has one).",
                file.display()
            )),
        });
    }
    for item in prepared_array {
        validate_type_hierarchy_item(item)?;
    }
    let item = &prepared_array[0];
    let mut seen = HashSet::new();
    let root_item_start = hierarchy_start(item);
    let root_line = root_item_start
        .and_then(|s| s.get("line"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let root_col = root_item_start
        .and_then(|s| s.get("character"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let root_uri = item.get("uri").and_then(|u| u.as_str()).unwrap_or("");
    seen.insert(format!("{root_uri}:{root_line}:{root_col}"));
    let list = expand_lsp_supertypes(remote, root, file, item.clone(), 1, depth, &mut seen).await?;
    Ok(Supertypes {
        of: item
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("?")
            .to_string(),
        kind: Kind::Other,
        list,
        depth,
        unsupported: None,
    })
}

fn expand_lsp_supertypes<'a>(
    remote: SocketAddr,
    root: &'a Path,
    file: &'a Path,
    item: serde_json::Value,
    level: usize,
    max_depth: usize,
    seen: &'a mut HashSet<String>,
) -> Pin<Box<dyn Future<Output = Result<Vec<Supertype>>> + Send + 'a>> {
    Box::pin(async move {
        if level > max_depth {
            return Ok(Vec::new());
        }
        let supers = execute_lsp_query(
            remote,
            root,
            file,
            "typeHierarchy/supertypes",
            serde_json::json!({ "item": item }),
        )
        .await?;
        let supers_array = match &supers {
            serde_json::Value::Null => return Ok(Vec::new()),
            serde_json::Value::Array(a) => a,
            other => anyhow::bail!(
                "the analyzer's typeHierarchy/supertypes answer is not an array or null: {other}"
            ),
        };
        let mut list = Vec::with_capacity(supers_array.len());
        for s in supers_array {
            validate_type_hierarchy_item(s)?;
            let name = s.get("name").and_then(|n| n.as_str()).unwrap().to_string();
            let uri = s.get("uri").and_then(|u| u.as_str()).unwrap();
            let start = hierarchy_start(s).unwrap();
            let line = start.get("line").and_then(|v| v.as_u64()).unwrap() as u32 + 1;
            let col = start.get("character").and_then(|v| v.as_u64()).unwrap() as u32 + 1;
            let path = PathBuf::from(crate::remote_fs::uri_to_path(uri));
            let mut node = Supertype::new(name, false, Some((path, line, col)));
            let key = format!("{uri}:{line}:{col}");
            if !seen.insert(key.clone()) {
                node.repeated = true;
            } else if level < max_depth {
                node.children = expand_lsp_supertypes(
                    remote,
                    root,
                    file,
                    s.clone(),
                    level + 1,
                    max_depth,
                    seen,
                )
                .await?;
            }
            list.push(node);
        }
        Ok(list)
    })
}
