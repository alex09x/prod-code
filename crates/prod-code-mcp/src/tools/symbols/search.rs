/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::matching::bare_symbol_name;
use super::nested_projects::representative_source_file;
use super::types::{MalformedLspCoordinate, SymbolHit, lsp_position, symbol_kind_name};
use crate::tools::execute_lsp_query;
use anyhow::Result;
use std::net::SocketAddr;
use std::path::Path;
use url::Url;

/// `workspace/symbol` through the pooled session of the project `hint` belongs to (the root
/// when absent). Hits without a range (LSP `WorkspaceSymbol` without resolve) are skipped.
pub async fn workspace_symbol_search(
    remote: SocketAddr,
    root: &Path,
    query: &str,
    hint: Option<&Path>,
    limit: usize,
) -> Result<Vec<SymbolHit>> {
    workspace_symbol_search_with_retry_policy(remote, root, query, hint, limit, true).await
}

pub(crate) async fn workspace_symbol_search_auxiliary(
    remote: SocketAddr,
    root: &Path,
    query: &str,
    hint: Option<&Path>,
    limit: usize,
) -> Result<Vec<SymbolHit>> {
    workspace_symbol_search_with_retry_policy(remote, root, query, hint, limit, false).await
}

pub(crate) async fn workspace_symbol_search_with_retry_policy(
    remote: SocketAddr,
    root: &Path,
    query: &str,
    hint: Option<&Path>,
    limit: usize,
    retry_empty_answer: bool,
) -> Result<Vec<SymbolHit>> {
    // The LSP servers (tsc, clangd, pyright) index a project once one of its files is open;
    // the session opens the anchor file before the query, so pick a real source file when the
    // caller gave none or a directory.
    let anchor = match hint {
        Some(h) if h.is_file() => h.to_path_buf(),
        Some(h) => representative_source_file(h).unwrap_or_else(|| h.to_path_buf()),
        None => representative_source_file(root).unwrap_or_else(|| root.to_path_buf()),
    };
    let params = serde_json::json!({ "query": query, "limit": limit.max(1) });
    let asked = std::time::Instant::now();
    let mut res =
        execute_lsp_query(remote, root, &anchor, "workspace/symbol", params.clone()).await?;
    // How patient to be with an empty answer depends on how long before the question the engine
    // was loaded (#381). A warm engine's empty answer is the answer: a miss used to sleep 800 ms
    // in every project it asked. One loaded moments before may still be indexing and is asked
    // again a few times, longer each time; a gateway that does not say gets the one retry it
    // always had. The age is taken at the question: a fresh gopls on a busy node took a minute
    // to give its first answer, and was no warmer for it.
    let age = crate::session::pooled_engine_age(remote, root, &anchor)
        .await
        .map(|age| age.saturating_sub(asked.elapsed()));
    // A gateway that holds index questions until the server is ready has already waited: its
    // empty answer is final (#391). The age is the guess for the others.
    let gated = crate::session::pooled_index_gated(remote, root, &anchor).await;
    let retries = match age {
        _ if gated => 0,
        _ if !retry_empty_answer => 0,
        Some(age) if age >= crate::session::INDEXING_GRACE => 0,
        Some(_) => 3,
        None => 1,
    };
    let mut pause = std::time::Duration::from_millis(800);
    for _ in 0..retries {
        if res.as_array().is_some_and(|a| !a.is_empty()) {
            break;
        }
        tokio::time::sleep(pause).await;
        pause *= 2;
        res = execute_lsp_query(remote, root, &anchor, "workspace/symbol", params.clone()).await?;
    }
    let mut hits = Vec::new();
    for sym in res.as_array().into_iter().flatten() {
        let Some(name) = sym.get("name").and_then(|n| n.as_str()) else {
            continue;
        };
        let context = format!("workspace symbol `{name}`");
        let Some(uri) = sym.pointer("/location/uri").and_then(|u| u.as_str()) else {
            if bare_symbol_name(name).eq_ignore_ascii_case(bare_symbol_name(query)) {
                return Err(anyhow::Error::new(MalformedLspCoordinate(format!(
                    "malformed LSP {context}: missing location URI"
                ))));
            }
            continue;
        };
        let Some(path) = Url::parse(uri).ok().and_then(|u| u.to_file_path().ok()) else {
            if bare_symbol_name(name).eq_ignore_ascii_case(bare_symbol_name(query)) {
                return Err(anyhow::Error::new(MalformedLspCoordinate(format!(
                    "malformed LSP {context}: invalid location URI"
                ))));
            }
            continue;
        };
        let Some(start) = sym.pointer("/location/range/start") else {
            if bare_symbol_name(name).eq_ignore_ascii_case(bare_symbol_name(query)) {
                return Err(anyhow::Error::new(MalformedLspCoordinate(format!(
                    "malformed LSP {context}: missing location range start"
                ))));
            }
            continue;
        };
        let matching = bare_symbol_name(name).eq_ignore_ascii_case(bare_symbol_name(query));
        let (line, col) = match lsp_position(start, &context) {
            Ok(position) => position,
            Err(error) if matching => return Err(error),
            Err(_) => continue,
        };
        hits.push(SymbolHit {
            path,
            name: name.to_string(),
            kind: symbol_kind_name(sym.get("kind").and_then(|k| k.as_u64()).unwrap_or(0)),
            container: sym
                .get("containerName")
                .and_then(|c| c.as_str())
                .filter(|c| !c.is_empty())
                .map(str::to_string),
            line,
            col,
        });
        if hits.len() >= limit {
            break;
        }
    }
    Ok(hits)
}
