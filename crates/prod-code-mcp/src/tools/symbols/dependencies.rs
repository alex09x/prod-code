/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::matching::qualifier_matches;
use super::nested_projects::representative_source_file;
use super::search::parse_workspace_symbol_hits;
use super::types::{SymbolHit, is_malformed_lsp_coordinate};
use anyhow::Result;
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

const DEPENDENCY_SEARCH_BUDGET: Duration = Duration::from_secs(5);

/// One bounded dependency-only request, after every workspace declaration fallback missed.
/// No indexing retries: the extra search must not prolong a missing-symbol response.
pub(super) async fn dependency_symbol_search(
    remote: SocketAddr,
    root: &Path,
    symbol: &str,
    owners: &[&str],
    hint: Option<&Path>,
) -> Result<Vec<SymbolHit>> {
    let anchor = hint
        .and_then(|path| {
            if path.is_file() {
                Some(path.to_path_buf())
            } else {
                representative_source_file(path)
            }
        })
        .or_else(|| representative_source_file(root))
        .unwrap_or_else(|| root.to_path_buf());
    if anchor.extension().is_none_or(|extension| extension != "rs") {
        return Ok(Vec::new());
    }
    let search = async {
        let response = crate::tools::execute_lsp_query(
            remote,
            root,
            &anchor,
            "workspace/symbol",
            serde_json::json!({
                "query": symbol,
                "limit": 200,
                "searchScope": "dependencies",
                "prodCodeTimeoutMs": DEPENDENCY_SEARCH_BUDGET.as_millis() as u64,
            }),
        )
        .await?;
        let mut hits = parse_workspace_symbol_hits(root, symbol, &response, 200)?;
        hits.retain(|hit| qualifier_matches(root, hit, owners));
        Ok(hits)
    };
    match tokio::time::timeout(DEPENDENCY_SEARCH_BUDGET, search).await {
        Ok(Err(error)) if is_malformed_lsp_coordinate(&error) => Err(error),
        Ok(Ok(hits)) => Ok(hits),
        _ => Ok(Vec::new()),
    }
}

#[cfg(test)]
mod tests;
