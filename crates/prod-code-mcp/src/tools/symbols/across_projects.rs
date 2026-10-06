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
use super::nested_projects::{
    MAX_NESTED_PROJECTS, MAX_SCANNED_FILES, nested_project_anchors, source_files,
};
use super::search::{workspace_symbol_search, workspace_symbol_search_auxiliary};
use super::types::{
    MalformedLspCoordinate, SymbolHit, is_malformed_lsp_coordinate, lsp_position, symbol_kind_name,
};
use super::unindexed::declared_at;
use crate::tools::execute_lsp_query;
use anyhow::Result;
use std::net::SocketAddr;
use std::path::Path;
use url::Url;

/// The most time allowed for cross-project symbol search before returning accumulated hits (#829).
pub(crate) const SYMBOL_SEARCH_BUDGET: std::time::Duration = std::time::Duration::from_secs(30);

/// The most files of a project read for the declarations of a name its server has no index of.
pub(crate) const MAX_OUTLINED_FILES: usize = 8;

/// Maximum bytes read from any one source while searching for names synchronously.
pub(crate) const MAX_NAME_SCAN_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// `workspace/symbol` in the checkout's project and, when that knows no symbol of the name and
/// no hint names a project, in the checkout's nested projects of other languages too (#318):
/// a Swift file in a Go module is in no index gopls keeps.
pub(crate) async fn symbol_search_across_projects(
    remote: SocketAddr,
    root: &Path,
    query: &str,
    hint: Option<&Path>,
    limit: usize,
) -> Result<Vec<SymbolHit>> {
    let deadline = tokio::time::Instant::now() + SYMBOL_SEARCH_BUDGET;
    let mut hits = match tokio::time::timeout_at(
        deadline,
        workspace_symbol_search(remote, root, query, hint, limit),
    )
    .await
    {
        Ok(result) => result?,
        Err(_) => {
            tracing::warn!(
                query,
                "primary workspace symbol search reached budget; returning no hits"
            );
            return Ok(Vec::new());
        }
    };
    let name = bare_symbol_name(query);
    let named = |hits: &[SymbolHit]| {
        hits.iter()
            .any(|hit| bare_symbol_name(&hit.name).eq_ignore_ascii_case(name))
    };
    if named(&hits) {
        return Ok(hits);
    }
    if let Some(hint) = hint {
        // The project a path names may keep no index (sourcekit-lsp before a build): the
        // outlines of its files that name the symbol still find it (#358).
        if let (_, Some(engine)) = crate::sync::engine_project(root, hint) {
            let node = match tokio::time::timeout_at(
                deadline,
                crate::cluster::route_for_path(remote, root, hint.to_str()),
            )
            .await
            {
                Ok(Ok(n)) => n,
                Ok(Err(_)) => remote,
                Err(_) => return Ok(hits),
            };
            let files = if hint.is_file() {
                read_name_scan_text(hint)
                    .is_some_and(|text| names_word(&text, name))
                    .then(|| hint.to_path_buf())
                    .into_iter()
                    .collect()
            } else {
                files_naming(hint, engine, name, deadline)
            };
            if !files.is_empty()
                && let Ok(Ok(decls)) =
                    tokio::time::timeout_at(deadline, declarations_in(node, root, &files, name))
                        .await
            {
                hits.extend(decls);
            }
        }
        return Ok(hits);
    }
    // The projects whose sources name the symbol first, then the others a walk meets (#358).
    // If the root project already returned relevant matches (prefix, word-boundary, substring),
    // and no nested project's sources explicitly name the query, avoid falling back to
    // arbitrary nested project anchors which can cause runaway timeouts (#829).
    let has_relevant_hits = hits.iter().any(|hit| match_rank(&hit.name, query) < 4);
    let mut anchors = projects_naming(root, name, deadline);
    if anchors.is_empty() {
        if has_relevant_hits {
            return Ok(hits);
        }
        for anchor in nested_project_anchors(root, deadline) {
            if anchors.len() >= MAX_NESTED_PROJECTS {
                break;
            }
            if !anchors
                .iter()
                .any(|(_, subpath, engine)| *subpath == anchor.1 && *engine == anchor.2)
            {
                anchors.push(anchor);
            }
        }
    }
    for (anchor, subpath, engine) in anchors {
        if tokio::time::Instant::now() >= deadline {
            tracing::warn!(
                query,
                "symbol search across projects reached query budget; returning accumulated hits"
            );
            break;
        }
        // Its own node: the checkout's may not serve its language (a Swift file needs a macOS
        // node, a Go module is placed on Linux).
        let node = match tokio::time::timeout_at(
            deadline,
            crate::cluster::route_for_path(remote, root, anchor.to_str()),
        )
        .await
        {
            Ok(Ok(n)) => n,
            Ok(Err(_)) => remote,
            Err(_) => {
                tracing::warn!(
                    query,
                    "symbol search route_for_path reached budget; returning accumulated hits"
                );
                break;
            }
        };

        if tokio::time::Instant::now() >= deadline {
            break;
        }

        let search_fut = workspace_symbol_search_auxiliary(node, root, query, Some(&anchor), limit);
        let found = match tokio::time::timeout_at(deadline, search_fut).await {
            Ok(Ok(found)) => found,
            Ok(Err(err)) => {
                tracing::debug!(
                    anchor = %anchor.display(),
                    error = %format!("{err:#}"),
                    "a nested project's symbol search failed"
                );
                if is_malformed_lsp_coordinate(&err) {
                    return Err(err);
                }
                Vec::new()
            }
            Err(_) => {
                tracing::warn!(
                    query,
                    "nested workspace_symbol_search reached budget; returning accumulated hits"
                );
                break;
            }
        };
        let named_here = found
            .iter()
            .any(|hit| bare_symbol_name(&hit.name).eq_ignore_ascii_case(name));
        hits.extend(found);
        if !named_here && tokio::time::Instant::now() < deadline {
            // sourcekit-lsp has no index for Swift files a package does not build, and answers
            // `workspace/symbol` with nothing: their outlines still name what they declare.
            let files = files_naming(&root.join(&subpath), engine, name, deadline);
            if !files.is_empty() {
                match tokio::time::timeout_at(deadline, declarations_in(node, root, &files, name))
                    .await
                {
                    Ok(Ok(decls)) => hits.extend(decls),
                    Ok(Err(e)) => return Err(e),
                    Err(_) => {
                        tracing::warn!(
                            query,
                            "declarations_in reached budget; returning accumulated hits"
                        );
                        break;
                    }
                }
            }
        }
        if named(&hits) {
            break;
        }
    }
    Ok(hits)
}

/// Files of `engine`'s language under `dir` whose text has `name` as a word, prioritizing files
/// that declare `name`.
pub(crate) fn files_naming(
    dir: &Path,
    engine: &str,
    name: &str,
    deadline: tokio::time::Instant,
) -> Vec<std::path::PathBuf> {
    let mut files: Vec<(bool, std::path::PathBuf)> = Vec::new();
    for path in source_files(dir).filter(|path| crate::sync::engine_for_file(path) == Some(engine))
    {
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        let Some(text) = read_name_scan_text(&path) else {
            continue;
        };
        if !names_word(&text, name) {
            continue;
        }
        let has_decl = text.lines().any(|line| declared_at(line, name).is_some());
        files.push((has_decl, path));
        if files.len() >= MAX_OUTLINED_FILES * 2 {
            break;
        }
    }
    files.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    files
        .into_iter()
        .map(|(_, p)| p)
        .take(MAX_OUTLINED_FILES)
        .collect()
}

/// Whether `text` has `name` as a whole word.
pub(crate) fn names_word(text: &str, name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    text.split(|c: char| !is_word(c))
        .any(|word| word.eq_ignore_ascii_case(name))
}

/// Reads only the prefix needed for bounded symbol-name discovery. Large generated sources do
/// not get to exceed the overall search budget through one unbounded synchronous read.
pub(crate) fn read_name_scan_text(path: &Path) -> Option<String> {
    use std::io::Read;

    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(MAX_NAME_SCAN_FILE_BYTES)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// One file of each nested project whose sources name `name`, in path order: the projects to
/// ask first (#358). A walk that takes the first projects it meets spent every slot on a C++
/// dependency and loose scripts while the declaration sat in a Swift package after them.
pub(crate) fn projects_naming(
    root: &Path,
    name: &str,
    deadline: tokio::time::Instant,
) -> Vec<(std::path::PathBuf, String, &'static str)> {
    let root_engine = crate::sync::expected_engine(root);
    let mut projects = std::collections::HashSet::new();
    let mut anchors = Vec::new();
    for path in source_files(root)
        .filter(|path| crate::sync::engine_for_file(path).is_some_and(|e| Some(e) != root_engine))
        .take(MAX_SCANNED_FILES)
    {
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        if !read_name_scan_text(&path).is_some_and(|text| names_word(&text, name)) {
            continue;
        }
        if let (Some(subpath), Some(engine)) = crate::sync::engine_project(root, &path)
            && projects.insert((subpath.clone(), engine))
        {
            anchors.push((path, subpath, engine));
            if anchors.len() >= MAX_NESTED_PROJECTS {
                break;
            }
        }
    }
    anchors
}

/// The declarations called `name` in the outlines of `files`.
pub(crate) async fn declarations_in(
    remote: SocketAddr,
    root: &Path,
    files: &[std::path::PathBuf],
    name: &str,
) -> Result<Vec<SymbolHit>> {
    let mut hits = Vec::new();
    for file in files {
        let Ok(uri) = Url::from_file_path(file) else {
            continue;
        };
        let params = serde_json::json!({ "textDocument": { "uri": uri.to_string() } });
        let Ok(outline) =
            execute_lsp_query(remote, root, file, "textDocument/documentSymbol", params).await
        else {
            continue;
        };
        collect_named(&outline, name, None, file, &mut hits)?;
        if hits
            .iter()
            .any(|hit| bare_symbol_name(&hit.name).eq_ignore_ascii_case(name))
        {
            break;
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
        if bare_symbol_name(own).eq_ignore_ascii_case(name) {
            let start = symbol
                .pointer("/selectionRange/start")
                .or_else(|| symbol.pointer("/location/range/start"));
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
            } else {
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
