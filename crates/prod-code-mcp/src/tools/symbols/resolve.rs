/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::across_projects::symbol_search_across_projects;
use super::matching::{bare_symbol_name, no_symbol_message, qualifier_matches, single_candidate};
use super::sources::{
    find_identifier_on_line, identifier_at, is_extension_declaration, is_use_declaration,
    remote_sources, source_text,
};
use super::type_members::type_members;
use super::types::SymbolHit;
use super::unindexed::{find_unindexed_declarations, unindexed_declarations, unindexed_members};
use anyhow::Result;
use std::net::SocketAddr;
use std::path::Path;

/// Resolves a (possibly qualified) symbol name to one position. Only a hit whose name is the
/// requested name (ignoring ASCII case and the server's decoration, see `bare_symbol_name`)
/// is a candidate: the index also returns prefix and fuzzy matches, and acting on one of those
/// renames or deletes a symbol the caller never named (#253). Qualifiers (`Type::name`,
/// `pkg.Func`, `Class.method`) are matched against the hit's container, `hint` (a file or
/// directory) prefers hits under it. A member the index does not list, such as a Rust struct
/// field, is found through its type's outline. A tie between different locations is an error
/// listing the candidates; no match is an error listing the closest names.
pub async fn resolve_symbol(
    remote: SocketAddr,
    root: &Path,
    symbol: &str,
    hint: Option<&Path>,
) -> Result<SymbolHit> {
    let parts: Vec<&str> = symbol
        .split(['.', ':', '#', '/'])
        .map(|p| p.trim().trim_end_matches("()"))
        .filter(|p| !p.is_empty())
        .collect();
    let name = bare_symbol_name(parts.last().copied().unwrap_or(symbol));
    let qualifier = parts.len().checked_sub(2).map(|i| parts[i]);
    // Every qualifying segment, for a path (`crate::module::item`) the hit's file spells out.
    let qualifiers = &parts[..parts.len().saturating_sub(1)];
    let exact: Vec<SymbolHit> = if qualifier.is_some() {
        // Fast-path: When looking up a qualified symbol (`Type::member`), resolve via the
        // owning type's outline first. The type itself (`qualifiers.last()`) is almost always
        // unique or has very few candidates, whereas common method names (`new`, `get`, `run`, `init`)
        // return 200+ unrelated candidates across the entire workspace and lead to expensive scans.
        let members = type_members(remote, root, qualifiers, name, hint)
            .await
            .unwrap_or_default();
        if !members.is_empty() {
            members
        } else {
            let hits = symbol_search_across_projects(remote, root, name, hint, 200).await?;
            let (exact, others): (Vec<SymbolHit>, Vec<SymbolHit>) = hits
                .into_iter()
                .partition(|hit| bare_symbol_name(&hit.name).eq_ignore_ascii_case(name));
            let eligible: Vec<SymbolHit> = exact
                .into_iter()
                .filter(|hit| qualifier_matches(root, hit, qualifiers))
                .collect();
            if !eligible.is_empty() {
                eligible
            } else {
                let stdlib_hits = find_stdlib_or_usage_hits(root, symbol, qualifiers, name, hint);
                if !stdlib_hits.is_empty() {
                    stdlib_hits
                } else {
                    anyhow::bail!("{}", no_symbol_message(symbol, name, &others));
                }
            }
        }
    } else {
        let hits = symbol_search_across_projects(remote, root, name, hint, 200).await?;
        let (exact, others): (Vec<SymbolHit>, Vec<SymbolHit>) = hits
            .into_iter()
            .partition(|hit| bare_symbol_name(&hit.name).eq_ignore_ascii_case(name));
        if exact.is_empty() {
            let unindexed_hits = find_unindexed_declarations(root, name, hint).await;
            if !unindexed_hits.is_empty() {
                unindexed_hits
            } else {
                let members = unindexed_members(remote, root, name, hint).await?;
                if !members.is_empty() {
                    members
                } else {
                    let unindexed = unindexed_declarations(remote, root, name).await;
                    anyhow::bail!("{}{unindexed}", no_symbol_message(symbol, name, &others));
                }
            }
        } else {
            exact
        }
    };
    let remote_texts = remote_sources(remote, &exact).await;
    // An explicit owner is not enough when the index or outline is stale. Qualified lookup
    // must point at the requested name in readable source, not merely rank that hit lower.
    let exact = if qualifier.is_some() {
        let mut verified: Vec<SymbolHit> = Vec::new();
        for mut hit in exact {
            let bare = bare_symbol_name(&hit.name);
            let raw_bare = bare.split(['(', '<']).next().unwrap_or(bare);
            let matches_exact =
                identifier_at(&hit.path, &remote_texts, hit.line, hit.col, &hit.name)
                    || identifier_at(&hit.path, &remote_texts, hit.line, hit.col, bare);
            if matches_exact {
                if let Some(text) = source_text(&hit.path, &remote_texts)
                    && let Some(row) = text.lines().nth(hit.line.saturating_sub(1) as usize)
                    && let Some(col_idx) = find_identifier_on_line(row, raw_bare)
                {
                    let char_col = row[..col_idx].chars().count() as u32 + 1;
                    hit.col = char_col;
                }
                verified.push(hit);
            }
        }
        anyhow::ensure!(
            !verified.is_empty(),
            "no verified symbol named `{symbol}`: the index or outline positions do not match readable current source; refresh the project index or supply a current source position"
        );
        verified
    } else {
        exact
    };
    let hint_str = hint.map(|h| h.to_string_lossy().into_owned());
    let mut scored: Vec<(i32, SymbolHit)> = exact
        .into_iter()
        .map(|hit| {
            let bare = bare_symbol_name(&hit.name);
            let mut score = if bare == name { 100 } else { 60 };
            if qualifier.is_some() {
                // Qualified hits already passed `qualifier_matches`; the score is only for
                // ordinary tie-breakers such as a caller's path hint.
            } else if hit.kind == "EnumMember" {
                // A bare name is the type's, not an enum's variant of the same name, which Rust
                // reaches as `Enum::Variant` (#325).
                score -= 20;
            }
            if let Some(h) = &hint_str {
                let p = hit.path.to_string_lossy();
                if *p == **h || p.starts_with(h.as_str()) {
                    score += 30;
                }
            }
            // The checkout's own symbol before a same-named one of the standard library or a
            // dependency, which gopls lists alongside (#345).
            if hit.path.starts_with(root) {
                score += 5;
            }
            // An index can be stale or (clangd) point the right range at the wrong file:
            // the name must actually be at that position. A server that decorates the name
            // (`Type.name`) may point at the bare name, so either spelling counts.
            if !identifier_at(&hit.path, &remote_texts, hit.line, hit.col, &hit.name)
                && !identifier_at(&hit.path, &remote_texts, hit.line, hit.col, bare)
            {
                score -= 1000;
            }
            (score, hit)
        })
        .collect();
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.path.cmp(&b.1.path))
            .then_with(|| a.1.line.cmp(&b.1.line))
    });
    let Some(first) = scored.first() else {
        anyhow::bail!("no symbol named `{symbol}` matches criteria");
    };
    let best = first.0;
    let ties: Vec<&SymbolHit> = scored
        .iter()
        .filter(|(s, _)| *s == best)
        .map(|(_, h)| h)
        .collect();
    // A re-export (`pub use sync::scan;`) is listed by the index next to the definition it
    // names. It is the same symbol, not a second candidate, and the answer is the definition.
    let definitions: Vec<&SymbolHit> = ties
        .iter()
        .copied()
        .filter(|h| !is_use_declaration(&h.path, &remote_texts, h.line))
        .collect();
    let ties = if definitions.is_empty() {
        ties
    } else {
        definitions
    };
    // So is a Swift `extension` of a type, listed under the type's name: the type's own
    // declaration is the answer (#358).
    let declarations: Vec<&SymbolHit> = ties
        .iter()
        .copied()
        .filter(|h| !is_extension_declaration(&h.path, &remote_texts, h.line))
        .collect();
    let ties = if declarations.is_empty() {
        ties
    } else {
        declarations
    };
    single_candidate(root, symbol, &ties)
}

/// Resolves standard library symbols (e.g. `std::process::Command`) or workspace usages when
/// not indexed by `workspace/symbol` (#935).
pub(crate) fn find_stdlib_or_usage_hits(
    root: &Path,
    symbol: &str,
    qualifiers: &[&str],
    name: &str,
    hint: Option<&Path>,
) -> Vec<SymbolHit> {
    let mut hits = Vec::new();
    let is_stdlib = matches!(qualifiers.first().copied(), Some("std" | "core" | "alloc"));
    if !is_stdlib && !symbol.contains("::") {
        return hits;
    }
    let mut files_to_check = Vec::new();
    if let Some(h) = hint {
        let p = if h.is_absolute() {
            h.to_path_buf()
        } else {
            root.join(h)
        };
        if p.is_file() {
            files_to_check.push(p);
        }
    }
    for file in files_to_check
        .into_iter()
        .chain(super::nested_projects::source_files(root).take(200))
    {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        if !text.contains(name) {
            continue;
        }
        for (idx, line) in text.lines().enumerate() {
            if line.contains(symbol)
                || (line.contains(name) && qualifiers.iter().any(|q| line.contains(q)))
            {
                if let Some(col) = find_identifier_on_line(line, name) {
                    hits.push(SymbolHit {
                        path: file.clone(),
                        name: name.to_string(),
                        kind: "Struct",
                        container: Some(qualifiers.join("::")),
                        line: idx as u32 + 1,
                        col: line[..col].chars().count() as u32 + 1,
                    });
                    return hits;
                }
            }
        }
    }
    hits
}
