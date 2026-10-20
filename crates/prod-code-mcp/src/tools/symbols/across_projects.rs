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
pub(crate) use super::nested_projects::{names_word, read_name_scan_text};
use super::nested_projects::{nested_project_anchors, projects_naming, source_files};
pub(crate) use super::search::declarations_in;
use super::search::{workspace_symbol_search, workspace_symbol_search_auxiliary};
use super::types::{SymbolHit, is_malformed_lsp_coordinate};
use super::unindexed::declared_at;
use anyhow::Result;
use std::net::SocketAddr;
use std::path::Path;

/// The most time allowed for cross-project symbol search before returning accumulated hits (#829).
pub(crate) const SYMBOL_SEARCH_BUDGET: std::time::Duration = std::time::Duration::from_secs(20);

/// The most files of a project read for the declarations of a name its server has no index of.
pub(crate) const MAX_OUTLINED_FILES: usize = 8;

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
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }
    let deadline = tokio::time::Instant::now() + SYMBOL_SEARCH_BUDGET;
    let parts: Vec<&str> = query
        .split([':', '.', '#', '/'])
        .map(|p| p.trim().trim_end_matches("()"))
        .filter(|p| !p.is_empty())
        .collect();
    let qualifiers = &parts[..parts.len().saturating_sub(1)];
    let qualifier_word = qualifiers.last().copied();
    let name = bare_symbol_name(query);
    let resolved_hint = hint.map(|h| {
        if h.is_absolute() {
            h.to_path_buf()
        } else {
            root.join(h)
        }
    });
    let hint = resolved_hint.as_deref();
    let named = |hits: &[SymbolHit]| {
        hits.iter().any(|hit| {
            bare_symbol_name(&hit.name).eq_ignore_ascii_case(name)
                && (qualifiers.is_empty() || qualifier_matches(root, hit, qualifiers))
                && hint.is_none_or(|h| hit.path == h || hit.path.starts_with(h))
        })
    };

    // Fast-path for explicit file hints: if the caller pointed at a specific source file
    // that contains the symbol name, check its outline first (#1033, #1036).
    if let Some(hint_path) = hint
        && hint_path.is_file()
        && read_name_scan_text(hint_path).is_some_and(|text| names_word(&text, name))
    {
        let node = match tokio::time::timeout_at(
            deadline,
            crate::cluster::route_for_path(remote, root, hint_path.to_str()),
        )
        .await
        {
            Ok(Ok(n)) => n,
            _ => remote,
        };
        if let Ok(Ok(mut decls)) = tokio::time::timeout_at(
            deadline,
            declarations_in(node, root, &[hint_path.to_path_buf()], name),
        )
        .await
        {
            if !qualifiers.is_empty() {
                decls.retain(|d| qualifier_matches(root, d, qualifiers));
            }
            if named(&decls) {
                return Ok(decls);
            }
        }
    }

    let primary_node = if let Some(h) = hint {
        match tokio::time::timeout_at(
            deadline,
            crate::cluster::route_for_path(remote, root, h.to_str()),
        )
        .await
        {
            Ok(Ok(n)) => n,
            _ => remote,
        }
    } else {
        remote
    };

    let primary_deadline =
        deadline.min(tokio::time::Instant::now() + std::time::Duration::from_secs(10));
    let mut primary_timed_out = false;
    let mut hits = match tokio::time::timeout_at(
        primary_deadline,
        workspace_symbol_search(primary_node, root, query, hint, limit),
    )
    .await
    {
        Ok(result) => result?,
        Err(_) => {
            primary_timed_out = true;
            tracing::warn!(
                query,
                "primary workspace symbol search reached budget; continuing with local fallback"
            );
            Vec::new()
        }
    };
    let extend_unique = |hits: &mut Vec<SymbolHit>, decls: Vec<SymbolHit>| {
        for d in decls {
            if !qualifiers.is_empty() && !qualifier_matches(root, &d, qualifiers) {
                continue;
            }
            if !hits.iter().any(|h| h.path == d.path && h.line == d.line) {
                hits.push(d);
            }
        }
    };
    if primary_timed_out && hits.is_empty() {
        let fallback_decls =
            super::declared_at::scan_timeout_fallback_declarations(root, name, hint, limit);
        extend_unique(&mut hits, fallback_decls);
        if !hits.is_empty() {
            return Ok(hits);
        }
    }
    if let Some(hint) = hint {
        // The project a path names may keep no index (sourcekit-lsp before a build), or its
        // index may omit `override func` or associated declarations (#358, #1091, #1109).
        if let (_, Some(engine)) = crate::sync::engine_project(root, hint)
            && (!named(&hits) || engine == "swift")
        {
            let dir = if hint.is_dir() {
                Some(hint)
            } else {
                hint.parent()
            };
            let files = dir
                .map(|d| files_naming_qualified(d, engine, name, qualifier_word, deadline))
                .unwrap_or_default();
            if !files.is_empty()
                && let Ok(Ok(decls)) = tokio::time::timeout_at(
                    deadline,
                    declarations_in(primary_node, root, &files, name),
                )
                .await
            {
                extend_unique(&mut hits, decls);
            }
        }
        return Ok(hits);
    }
    if let Some(root_engine) = crate::sync::expected_engine(root)
        && (root_engine == "swift" || (!qualifiers.is_empty() && !named(&hits)))
        && tokio::time::Instant::now() < deadline
    {
        let decl_deadline =
            deadline.min(tokio::time::Instant::now() + std::time::Duration::from_secs(5));
        let files = files_naming_qualified(root, root_engine, name, qualifier_word, decl_deadline);
        if !files.is_empty()
            && let Ok(Ok(decls)) =
                tokio::time::timeout_at(decl_deadline, declarations_in(remote, root, &files, name))
                    .await
        {
            extend_unique(&mut hits, decls);
        }
    }
    if named(&hits) {
        return Ok(hits);
    }
    let has_relevant_hits = hits.iter().any(|hit| match_rank(&hit.name, query) < 4);
    if has_relevant_hits {
        return Ok(hits);
    }
    let mut anchors = projects_naming(root, name, deadline);
    if let Some(q) = qualifier_word {
        anchors.sort_by_key(|(path, _, _)| {
            !read_name_scan_text(path).is_some_and(|text| names_word(&text, q))
        });
    }
    if anchors.is_empty() {
        for anchor in nested_project_anchors(root, deadline) {
            if anchors.len() >= 2 {
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
        let step_budget = std::time::Duration::from_secs(4);
        let route_deadline = deadline.min(tokio::time::Instant::now() + step_budget);
        let node = match tokio::time::timeout_at(
            route_deadline,
            crate::cluster::route_for_path(remote, root, anchor.to_str()),
        )
        .await
        {
            Ok(Ok(n)) => n,
            Ok(Err(_)) => remote,
            Err(_) => {
                tracing::warn!(
                    query,
                    "symbol search route_for_path reached budget; continuing to next project"
                );
                continue;
            }
        };

        if tokio::time::Instant::now() >= deadline {
            break;
        }

        let search_deadline = deadline.min(tokio::time::Instant::now() + step_budget);
        let search_fut = workspace_symbol_search_auxiliary(node, root, query, Some(&anchor), limit);
        let found = match tokio::time::timeout_at(search_deadline, search_fut).await {
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
                    "nested workspace_symbol_search reached budget; continuing to next project"
                );
                Vec::new()
            }
        };
        let named_here = found.iter().any(|hit| {
            bare_symbol_name(&hit.name).eq_ignore_ascii_case(name)
                && (qualifiers.is_empty() || qualifier_matches(root, hit, qualifiers))
        });
        hits.extend(found);
        if (!named_here || engine == "swift") && tokio::time::Instant::now() < deadline {
            let decl_deadline = deadline.min(tokio::time::Instant::now() + step_budget);
            let files = files_naming_qualified(
                &root.join(&subpath),
                engine,
                name,
                qualifier_word,
                decl_deadline,
            );
            if !files.is_empty() {
                match tokio::time::timeout_at(
                    decl_deadline,
                    declarations_in(node, root, &files, name),
                )
                .await
                {
                    Ok(Ok(decls)) => extend_unique(&mut hits, decls),
                    Ok(Err(e)) => return Err(e),
                    Err(_) => {
                        tracing::warn!(
                            query,
                            "declarations_in reached project budget; continuing to next project"
                        );
                        continue;
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
/// that declare `name` (and optionally also mention `qualifier`).
pub(crate) fn files_naming_qualified(
    dir: &Path,
    engine: &str,
    name: &str,
    qualifier: Option<&str>,
    deadline: tokio::time::Instant,
) -> Vec<std::path::PathBuf> {
    let mut files: Vec<(bool, bool, std::path::PathBuf)> = Vec::new();
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
        let has_qual = qualifier.is_none_or(|q| names_word(&text, q));
        let has_decl = text.lines().any(|line| declared_at(line, name).is_some());
        files.push((has_qual, has_decl, path));
        if files.iter().filter(|(q, d, _)| *q && *d).count() >= MAX_OUTLINED_FILES {
            break;
        }
        if files.len() >= MAX_OUTLINED_FILES * 4 {
            break;
        }
    }
    files.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| b.1.cmp(&a.1))
            .then_with(|| a.2.cmp(&b.2))
    });
    let any_qual = files.iter().any(|(q, _, _)| *q);
    let any_decl = files
        .iter()
        .filter(|(q, _, _)| !any_qual || *q)
        .any(|(_, d, _)| *d);
    files
        .into_iter()
        .filter(|(q, d, _)| (!any_qual || *q) && (!any_decl || *d))
        .map(|(_, _, p)| p)
        .take(MAX_OUTLINED_FILES)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn whitespace_only_query_fast_bails_without_hitting_network() {
        let dummy_addr: SocketAddr = "127.0.0.1:9".parse().unwrap();
        let dummy_path = Path::new("/dummy");
        let result =
            symbol_search_across_projects(dummy_addr, dummy_path, "   \t \n  ", None, 30).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());

        let result2 = symbol_search_across_projects(dummy_addr, dummy_path, "", None, 30).await;
        assert!(result2.is_ok());
        assert!(result2.unwrap().is_empty());
    }
}
