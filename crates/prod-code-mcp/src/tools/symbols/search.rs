/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::matching::{bare_symbol_name, match_rank, qualifier_matches};
use super::nested_projects::{read_name_scan_text, representative_source_file};
use super::type_members::names_type;
use super::types::{MalformedLspCoordinate, SymbolHit, lsp_position, symbol_kind_name};
use super::unindexed::declared_at;
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
    let anchor = match hint {
        Some(h) if h.is_file() => h.to_path_buf(),
        Some(h) => representative_source_file(h).unwrap_or_else(|| h.to_path_buf()),
        None => representative_source_file(root).unwrap_or_else(|| root.to_path_buf()),
    };
    let params = serde_json::json!({ "query": query, "limit": limit.max(1) });
    let asked = std::time::Instant::now();
    let mut res =
        execute_lsp_query(remote, root, &anchor, "workspace/symbol", params.clone()).await?;
    let age = crate::session::pooled_engine_age(remote, root, &anchor)
        .await
        .map(|age| age.saturating_sub(asked.elapsed()));
    let gated = crate::session::pooled_index_gated(remote, root, &anchor).await;
    let retries = match age {
        _ if gated => 0,
        _ if !retry_empty_answer => 0,
        Some(age) if age >= crate::session::INDEXING_GRACE => 0,
        Some(_) => 2,
        None => 1,
    };
    let mut pause = std::time::Duration::from_millis(600);
    for _ in 0..retries {
        if res.as_array().is_some_and(|a| !a.is_empty()) {
            break;
        }
        tokio::time::sleep(pause).await;
        pause *= 2;
        res = execute_lsp_query(remote, root, &anchor, "workspace/symbol", params.clone()).await?;
    }
    let mut hits = parse_workspace_symbol_hits(root, query, &res, limit)?;
    let parts: Vec<&str> = query
        .split([':', '.', '#', '/'])
        .map(|p| p.trim().trim_end_matches("()"))
        .filter(|p| !p.is_empty())
        .collect();
    let qualifiers = &parts[..parts.len().saturating_sub(1)];
    if !qualifiers.is_empty() {
        hits.retain(|h| qualifier_matches(root, h, qualifiers));
        let bare = bare_symbol_name(query);
        if !hits
            .iter()
            .any(|h| bare_symbol_name(&h.name).eq_ignore_ascii_case(bare))
        {
            let bare_params = serde_json::json!({ "query": bare, "limit": limit.max(200) });
            if let Ok(bare_res) =
                execute_lsp_query(remote, root, &anchor, "workspace/symbol", bare_params).await
                && let Ok(bare_hits) =
                    parse_workspace_symbol_hits(root, bare, &bare_res, limit.max(200))
            {
                for hit in bare_hits {
                    if qualifier_matches(root, &hit, qualifiers)
                        && !hits
                            .iter()
                            .any(|h| h.path == hit.path && h.line == hit.line && h.col == hit.col)
                    {
                        hits.push(hit);
                        if hits.len() >= limit {
                            break;
                        }
                    }
                }
            }
        }
    }
    Ok(hits)
}

fn parse_workspace_symbol_hits(
    root: &Path,
    query: &str,
    res: &serde_json::Value,
    limit: usize,
) -> Result<Vec<SymbolHit>> {
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
        if super::nested_projects::is_in_nested_git_worktree(root, &path) {
            continue;
        }
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

/// The declarations called `name` in the outlines of `files`, falling back to local source
/// declaration extraction when the language server has no outline for `file` (#1107, #1109).
pub(crate) async fn declarations_in(
    remote: SocketAddr,
    root: &Path,
    files: &[std::path::PathBuf],
    name: &str,
) -> Result<Vec<SymbolHit>> {
    let mut hits = Vec::new();
    for file in files {
        let mut file_hits = Vec::new();
        let mut allow_local_fallback = remote.port() != 9;
        if let Ok(uri) = Url::from_file_path(file) {
            let params = serde_json::json!({ "textDocument": { "uri": uri.to_string() } });
            let query_fut =
                execute_lsp_query(remote, root, file, "textDocument/documentSymbol", params);
            match tokio::time::timeout(std::time::Duration::from_millis(2500), query_fut).await {
                Ok(Ok(outline)) => {
                    collect_named(&outline, name, None, file, &mut file_hits)?;
                    if file.extension().and_then(|e| e.to_str()) != Some("swift") {
                        allow_local_fallback = false;
                    }
                }
                Ok(Err(_)) | Err(_) => {}
            }
        }
        if file_hits.is_empty() && allow_local_fallback {
            file_hits = local_declarations_in_file(file, name);
        }
        for hit in file_hits {
            if !hits
                .iter()
                .any(|h: &SymbolHit| h.path == hit.path && h.line == hit.line)
            {
                hits.push(hit);
            }
        }
    }
    Ok(hits)
}

/// Walks a `textDocument/documentSymbol` answer, nested or flat, for the symbols called `name`.
pub(crate) fn collect_named(
    symbols: &serde_json::Value,
    name: &str,
    parent: Option<&str>,
    file: &Path,
    out: &mut Vec<SymbolHit>,
) -> Result<()> {
    for symbol in symbols.as_array().into_iter().flatten() {
        let own = symbol.get("name").and_then(|n| n.as_str()).unwrap_or("");
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
                    kind: symbol_kind_name(
                        symbol.get("kind").and_then(|k| k.as_u64()).unwrap_or(0),
                    ),
                    container: parent.map(str::to_string).or_else(|| {
                        symbol
                            .get("containerName")
                            .and_then(|c| c.as_str())
                            .filter(|c| !c.is_empty())
                            .map(str::to_string)
                    }),
                    line,
                    col,
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

pub(crate) fn local_declarations_in_file(file: &Path, name: &str) -> Vec<SymbolHit> {
    let Some(text) = read_name_scan_text(file) else {
        return Vec::new();
    };
    let mut hits = Vec::new();
    let mut current_container: Option<(String, i32)> = None;
    let mut brace_depth: i32 = 0;
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if !trimmed.starts_with("//") && !trimmed.starts_with('#') && !trimmed.starts_with('*') {
            if let Some(container_label) = enclosing_container_header(trimmed) {
                current_container = Some((container_label, brace_depth));
            }
            if let Some(col) = declared_at(line, name) {
                let kind = inferred_declaration_kind(trimmed, current_container.is_some());
                let container = current_container
                    .as_ref()
                    .and_then(|(c, _)| (!names_type(c, name)).then(|| c.clone()));
                hits.push(SymbolHit {
                    path: file.to_path_buf(),
                    name: name.to_string(),
                    kind,
                    container,
                    line: index as u32 + 1,
                    col: col as u32 + 1,
                });
            }
            for ch in trimmed.chars() {
                match ch {
                    '{' => brace_depth += 1,
                    '}' => {
                        brace_depth = (brace_depth - 1).max(0);
                        if current_container
                            .as_ref()
                            .is_some_and(|(_, start_depth)| brace_depth <= *start_depth)
                        {
                            current_container = None;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    hits
}

fn enclosing_container_header(trimmed: &str) -> Option<String> {
    if !trimmed.ends_with('{') && !trimmed.ends_with(':') {
        return None;
    }
    let body = trimmed.trim_end_matches(['{', ':']).trim();
    if body.starts_with("impl ") || body.starts_with("impl<") {
        return Some(body.to_string());
    }
    for kw in [
        "struct ",
        "class ",
        "enum ",
        "trait ",
        "protocol ",
        "actor ",
        "extension ",
        "interface ",
    ] {
        if let Some(pos) = body.find(kw)
            && (pos == 0 || body[..pos].ends_with(char::is_whitespace))
        {
            let rest = body[pos + kw.len()..].trim();
            let ident: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !ident.is_empty() {
                return Some(ident);
            }
        }
    }
    None
}

fn inferred_declaration_kind(trimmed: &str, in_container: bool) -> &'static str {
    for (kw, kind) in [
        ("struct ", "Struct"),
        ("class ", "Class"),
        ("enum ", "Enum"),
        ("trait ", "Interface"),
        ("protocol ", "Interface"),
        ("interface ", "Interface"),
        ("actor ", "Class"),
        ("type ", "TypeParameter"),
        ("typealias ", "TypeParameter"),
        ("const ", "Constant"),
        ("static ", "Constant"),
        ("mod ", "Module"),
    ] {
        if trimmed.starts_with(kw) || trimmed.contains(&format!(" {kw}")) {
            return kind;
        }
    }
    if trimmed.starts_with("fn ")
        || trimmed.contains(" fn ")
        || trimmed.starts_with("func ")
        || trimmed.contains(" func ")
        || trimmed.starts_with("def ")
        || trimmed.contains(" def ")
        || trimmed.starts_with("function ")
        || trimmed.contains(" function ")
    {
        return if in_container { "Method" } else { "Function" };
    }
    if in_container { "Field" } else { "Variable" }
}
