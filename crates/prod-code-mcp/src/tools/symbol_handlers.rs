/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result};
use std::net::SocketAddr;
use std::path::Path;

use super::resolve_file_path;
use super::symbols::{
    SymbolHit, match_rank, symbol_search_across_projects, unindexed_declarations,
};
use crate::protocol::McpToolCallResult;

pub(crate) async fn handle_symbols(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let query = args
        .get("query")
        .and_then(|v| v.as_str())
        .context("Missing 'query' argument")?;
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(30) as usize;
    let hint = args
        .get("path")
        .and_then(|v| v.as_str())
        .map(|p| resolve_file_path(workspace_root, p));
    // More than are shown: a server lists its fuzzy matches in its own order, and the names
    // that hold the query may come after the limit.
    let mut hits = symbol_search_across_projects(
        remote,
        workspace_root,
        query,
        hint.as_deref(),
        limit.max(SYMBOL_CANDIDATES),
    )
    .await?;
    if hits.is_empty() {
        let unindexed = unindexed_declarations(remote, workspace_root, query).await;
        return Ok(McpToolCallResult::text(format!(
            "No symbols match `{query}`.{unindexed}"
        )));
    }
    Ok(McpToolCallResult::text(render_symbol_hits(
        workspace_root,
        query,
        &mut hits,
        limit,
    )))
}

/// How many hits `code_symbols` asks the server for before ranking them.
const SYMBOL_CANDIDATES: usize = 100;

/// The hits of a name search, best matches first (#326). Names that only have the query's
/// letters in order are shown when there is nothing better, and said to be that.
fn render_symbol_hits(root: &Path, query: &str, hits: &mut Vec<SymbolHit>, limit: usize) -> String {
    hits.sort_by_key(|hit| match_rank(&hit.name, query));
    let fuzzy = hits
        .iter()
        .filter(|hit| match_rank(&hit.name, query) == 4)
        .count();
    let all_fuzzy = fuzzy == hits.len();
    let mut out = if all_fuzzy {
        format!(
            "No symbol is named like `{query}`; {} whose names have its letters in order:\n",
            hits.len().min(limit)
        )
    } else {
        hits.retain(|hit| match_rank(&hit.name, query) < 4);
        format!("{} symbol(s) matching `{query}`:\n", hits.len().min(limit))
    };
    for hit in hits.iter().take(limit) {
        out.push_str(&format!("  {}\n", hit.render(root)));
    }
    if fuzzy > 0 && !all_fuzzy {
        out.push_str(&format!(
            "  ({fuzzy} more only have its letters in order; not shown)\n"
        ));
    }
    out.trim_end().to_string()
}

pub(crate) async fn handle_migrate_type(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .or_else(|| args.get("file"))
        .and_then(|v| v.as_str());
    let symbol = args.get("symbol").and_then(|v| v.as_str());
    let line = args.get("line").and_then(|v| v.as_u64()).map(|v| v as u32);
    let character = args
        .get("character")
        .or_else(|| args.get("col"))
        .and_then(|v| v.as_u64())
        .map(|v| v as u32);
    let to = args
        .get("to")
        .and_then(|v| v.as_str())
        .context("Missing 'to' argument: the type it should become")?;
    let convert = args
        .get("convert")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let transitive = args
        .get("transitive")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);

    let file_path = if let Some(p) = path_str {
        resolve_file_path(workspace_root, p)
    } else if let Some(sym) = symbol {
        let clean_sym = sym
            .rsplit("::")
            .next()
            .unwrap_or(sym)
            .rsplit('.')
            .next()
            .unwrap_or(sym)
            .trim();
        let mut found = None;
        for entry in ignore::WalkBuilder::new(workspace_root).build().flatten() {
            let p = entry.path();
            if p.is_file()
                && let Ok(content) = std::fs::read_to_string(p)
                && content.contains(clean_sym)
            {
                found = Some(p.to_path_buf());
                break;
            }
        }
        found.with_context(|| format!("could not find file declaring symbol `{sym}`"))?
    } else {
        anyhow::bail!("Missing 'path' or 'symbol' argument");
    };

    let done = crate::type_migration::migrate_ext(
        remote,
        workspace_root,
        &file_path,
        symbol,
        line,
        character,
        to,
        convert,
        transitive,
        apply,
        force,
    )
    .await?;
    let clean = done.sites.is_empty();
    let text = done.render(40);
    Ok(if clean {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}
