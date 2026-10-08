/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::across_projects::{names_word, symbol_search_across_projects};
use super::matching::{bare_symbol_name, owner_path_matches, owner_segments};
use super::nested_projects::source_files;
use super::sources::{RemoteSources, is_use_declaration};
use super::types::{MalformedLspCoordinate, SymbolHit, lsp_position, symbol_kind_name};
use super::unindexed::declared_at;
use crate::tools::execute_lsp_query;
use anyhow::Result;
use std::net::SocketAddr;
use std::path::Path;
use url::Url;

pub(crate) const TYPE_MEMBERS_BUDGET: std::time::Duration = std::time::Duration::from_secs(12);

pub(crate) fn find_source_members(
    root: &Path,
    path: &Path,
    owner: &[&str],
    member: &str,
) -> Vec<SymbolHit> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let type_name = owner.last().copied().unwrap_or_default();
    if !names_word(&text, member) || (!type_name.is_empty() && !names_word(&text, type_name)) {
        return Vec::new();
    }
    if !type_name.is_empty() && !owner_path_matches(root, path, owner, &[type_name.to_string()]) {
        return Vec::new();
    }
    let mut hits = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if let Some(col) = declared_at(line, member) {
            hits.push(SymbolHit {
                path: path.to_path_buf(),
                name: member.to_string(),
                kind: "Property",
                container: Some(owner.join("::")),
                line: index as u32 + 1,
                col: col as u32 + 1,
            });
        }
    }
    hits
}

/// The members called `member` of the type called `type_name`, read from the outline of each
/// file that declares the type. The type is resolved the way a symbol is, by its exact name;
/// with a `hint`, members under it are preferred.
pub(crate) async fn type_members(
    remote: SocketAddr,
    root: &Path,
    owner: &[&str],
    member: &str,
    hint: Option<&Path>,
) -> Result<Vec<SymbolHit>> {
    let Some(type_name) = owner.last().copied() else {
        return Ok(Vec::new());
    };
    let deadline = tokio::time::Instant::now() + TYPE_MEMBERS_BUDGET;
    let types = symbol_search_across_projects(remote, root, type_name, hint, 50)
        .await
        .unwrap_or_default();
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    if let Some(h) = hint {
        let p = if h.is_absolute() {
            h.to_path_buf()
        } else {
            root.join(h)
        };
        if p.is_file() {
            files.push(p);
        } else if p.is_dir() {
            for entry in source_files(&p).take(8) {
                if std::fs::read_to_string(&entry).is_ok_and(|text| names_word(&text, member)) {
                    files.push(entry);
                }
            }
        }
    }
    for hit in &types {
        if bare_symbol_name(&hit.name).eq_ignore_ascii_case(type_name)
            && !is_use_declaration(&hit.path, &RemoteSources::new(), hit.line)
            && !files.contains(&hit.path)
        {
            files.push(hit.path.clone());
        }
    }
    let mut members: Vec<SymbolHit> = Vec::new();
    for file in files.iter().take(8) {
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        let Ok(uri) = Url::from_file_path(file) else {
            continue;
        };
        let params = serde_json::json!({ "textDocument": { "uri": uri.to_string() } });
        let query_fut =
            execute_lsp_query(remote, root, file, "textDocument/documentSymbol", params);
        let Ok(Ok(outline)) = tokio::time::timeout_at(
            deadline.min(tokio::time::Instant::now() + std::time::Duration::from_secs(3)),
            query_fut,
        )
        .await
        else {
            continue;
        };
        let mut found = Vec::new();
        collect_members(&outline, root, file, owner, member, &[], &mut found)?;
        for (name, kind, line, col) in found {
            let hit = SymbolHit {
                path: file.clone(),
                name,
                kind: symbol_kind_name(kind),
                container: Some(owner.join("::")),
                line,
                col,
            };
            if !members
                .iter()
                .any(|m| m.path == hit.path && m.line == hit.line && m.col == hit.col)
            {
                members.push(hit);
            }
        }
    }
    let mut candidates = Vec::new();
    if members.is_empty() {
        for path in source_files(root).take(1000) {
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            if !files.contains(&path)
                && std::fs::read_to_string(&path)
                    .is_ok_and(|text| names_word(&text, member) && names_word(&text, type_name))
            {
                candidates.push(path);
                if candidates.len() >= 4 {
                    break;
                }
            }
        }
        for file in &candidates {
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            let Ok(uri) = Url::from_file_path(file) else {
                continue;
            };
            let params = serde_json::json!({ "textDocument": { "uri": uri.to_string() } });
            let query_fut =
                execute_lsp_query(remote, root, file, "textDocument/documentSymbol", params);
            let Ok(Ok(outline)) = tokio::time::timeout_at(
                deadline.min(tokio::time::Instant::now() + std::time::Duration::from_secs(3)),
                query_fut,
            )
            .await
            else {
                continue;
            };
            let mut found = Vec::new();
            collect_members(&outline, root, file, owner, member, &[], &mut found)?;
            for (name, kind, line, col) in found {
                let hit = SymbolHit {
                    path: file.clone(),
                    name,
                    kind: symbol_kind_name(kind),
                    container: Some(owner.join("::")),
                    line,
                    col,
                };
                if !members
                    .iter()
                    .any(|m| m.path == hit.path && m.line == hit.line && m.col == hit.col)
                {
                    members.push(hit);
                }
            }
        }
    }
    if members.is_empty() {
        for file in files.iter().chain(&candidates) {
            for hit in find_source_members(root, file, owner, member) {
                if !members
                    .iter()
                    .any(|m| m.path == hit.path && m.line == hit.line && m.col == hit.col)
                {
                    members.push(hit);
                }
            }
        }
    }
    if members.is_empty() {
        if let Some(alias_target) = super::alias::find_type_alias_target(root, type_name, hint) {
            let mut alias_owner = owner.to_vec();
            if let Some(last) = alias_owner.last_mut() {
                *last = &alias_target;
            }
            if let Ok(mut alias_members) =
                Box::pin(type_members(remote, root, &alias_owner, member, None)).await
            {
                if !alias_members.is_empty() {
                    for m in &mut alias_members {
                        m.container = Some(owner.join("::"));
                    }
                    return Ok(alias_members);
                }
            }
        }
    }
    if let Some(h) = hint {
        let h_abs = if h.is_absolute() {
            h.to_path_buf()
        } else {
            root.join(h)
        };
        let under_hint =
            |m: &SymbolHit| m.path == h_abs || m.path.starts_with(&h_abs) || m.path.ends_with(h);
        if members.iter().any(under_hint) {
            members.retain(under_hint);
        }
    }
    Ok(members)
}

/// Walks a `textDocument/documentSymbol` answer for the members called `member` of the type
/// called `type_name`, as (name, LSP kind, 1-based line, 1-based column) of the member's name.
/// A nested answer lists them as children of the type, or of an `impl` block for it (that is
/// where rust-analyzer puts methods); a flat one, as the gateway's own engine answers, names the
/// parent in `containerName`, innermost last after ` > `.
pub(crate) fn collect_members(
    symbols: &serde_json::Value,
    root: &Path,
    path: &Path,
    owner: &[&str],
    member: &str,
    ancestors: &[String],
    out: &mut Vec<(String, u64, u32, u32)>,
) -> Result<()> {
    let type_name = owner.last().copied().unwrap_or_default();
    let is_member = |sym: &serde_json::Value| {
        let name = sym.get("name").and_then(|n| n.as_str()).unwrap_or("");
        bare_symbol_name(name).eq_ignore_ascii_case(member)
    };
    for sym in symbols.as_array().into_iter().flatten() {
        let parent = sym.get("containerName").and_then(|c| c.as_str());
        if is_member(sym)
            && parent.is_some_and(|parent| {
                owner_path_matches(root, path, owner, &owner_segments(parent))
            })
        {
            out.push(member_at(sym)?);
        }
        let Some(children) = sym.get("children") else {
            continue;
        };
        let name = sym.get("name").and_then(|n| n.as_str()).unwrap_or("");
        let mut declared = ancestors.to_vec();
        declared.extend(owner_segments(name));
        if names_type(name, type_name) && owner_path_matches(root, path, owner, &declared) {
            for child in children.as_array().into_iter().flatten() {
                if is_member(child) {
                    out.push(member_at(child)?);
                }
            }
        }
        let kind = sym.get("kind").and_then(|kind| kind.as_u64()).unwrap_or(0);
        let nested_ancestors = if matches!(kind, 2..=5 | 10 | 11 | 23) {
            declared
        } else {
            ancestors.to_vec()
        };
        collect_members(children, root, path, owner, member, &nested_ancestors, out)?;
    }
    Ok(())
}

/// A document symbol as (name, kind, line, column) of its name, 1-based: the selection range
/// when there is one, which is the name rather than the doc comment the range starts at.
pub(crate) fn member_at(sym: &serde_json::Value) -> Result<(String, u64, u32, u32)> {
    let start = sym
        .pointer("/selectionRange/start")
        .or_else(|| sym.pointer("/range/start"))
        .or_else(|| sym.pointer("/location/range/start"))
        .ok_or_else(|| {
            anyhow::Error::new(MalformedLspCoordinate(
                "malformed LSP member symbol: missing selection range start".to_string(),
            ))
        })?;
    let name = sym
        .get("name")
        .and_then(|name| name.as_str())
        .ok_or_else(|| {
            anyhow::Error::new(MalformedLspCoordinate(
                "malformed LSP member symbol: missing name".to_string(),
            ))
        })?;
    let (line, col) = lsp_position(start, &format!("member symbol `{name}`"))?;
    Ok((
        name.to_string(),
        sym.get("kind").and_then(|k| k.as_u64()).unwrap_or(0),
        line,
        col,
    ))
}

/// Whether an outline label names the type `type_name`: the type itself, or an `impl` block
/// for it (`impl Type`, `impl<T> Type<T>`, `impl Trait for Type`), whose members are the
/// type's too.
pub(crate) fn names_type(label: &str, type_name: &str) -> bool {
    owner_segments(label).last().is_some_and(|target| {
        target
            .replace('-', "_")
            .eq_ignore_ascii_case(&type_name.replace('-', "_"))
    })
}
