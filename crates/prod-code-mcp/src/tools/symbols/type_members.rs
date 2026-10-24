/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::across_projects::{names_word, read_name_scan_text, symbol_search_across_projects};
use super::matching::{
    bare_symbol_name, decorated_owner, is_test_source_path, names_type, owner_path_matches,
    owner_segments,
};
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

pub(crate) use super::source_members::find_source_members;

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
    if types.iter().any(|hit| {
        hit.path.extension().is_some_and(|ext| ext == "rs")
            && bare_symbol_name(&hit.name).eq_ignore_ascii_case(type_name)
            && matches!(hit.kind, "Module" | "Namespace")
    }) && !types.iter().any(|hit| {
        bare_symbol_name(&hit.name).eq_ignore_ascii_case(type_name)
            && matches!(hit.kind, "Struct" | "Class" | "Enum" | "Interface")
    }) {
        return Ok(Vec::new());
    }
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
                if read_name_scan_text(&entry).is_some_and(|text| names_word(&text, member)) {
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
        // First inspect sibling files in the same directory/package as the type declaration (#1110).
        for type_file in &files {
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            let type_engine = crate::sync::engine_for_file(type_file).filter(|e| !e.is_empty());
            let (Some(type_engine), Some(parent_dir)) = (type_engine, type_file.parent()) else {
                continue;
            };
            let Ok(entries) = std::fs::read_dir(parent_dir) else {
                continue;
            };
            let mut siblings: Vec<std::path::PathBuf> = entries
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.is_file()
                        && !files.contains(p)
                        && crate::sync::engine_for_file(p) == Some(type_engine)
                })
                .collect();
            siblings.sort_by_key(|p| (is_test_source_path(p), p.clone()));
            for sibling in siblings {
                if tokio::time::Instant::now() >= deadline {
                    break;
                }
                if candidates.contains(&sibling) {
                    continue;
                }
                if read_name_scan_text(&sibling)
                    .is_some_and(|text| names_word(&text, member) && names_word(&text, type_name))
                {
                    candidates.push(sibling);
                    if candidates.len() >= 6 {
                        break;
                    }
                }
            }
        }
        if candidates.len() < 4 {
            for path in source_files(root).take(1000) {
                if tokio::time::Instant::now() >= deadline {
                    break;
                }
                if !files.contains(&path)
                    && !candidates.contains(&path)
                    && !is_test_source_path(&path)
                    && read_name_scan_text(&path).is_some_and(|text| {
                        names_word(&text, member)
                            && names_word(&text, type_name)
                            && text.lines().any(|l| declared_at(l, member).is_some())
                    })
                {
                    candidates.push(path);
                    if candidates.len() >= 6 {
                        break;
                    }
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
/// parent in `containerName`, innermost last after ` > `; `gopls` names receiver methods at top
/// level as `(*Type).Method` or `(Type).Method` (#1110).
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
        let name = sym.get("name").and_then(|n| n.as_str()).unwrap_or("");
        let parent = sym.get("containerName").and_then(|c| c.as_str());
        let decorated = decorated_owner(name);
        if is_member(sym)
            && (parent.is_some_and(|parent| {
                owner_path_matches(root, path, owner, &owner_segments(parent))
            }) || (!decorated.is_empty() && owner_path_matches(root, path, owner, &decorated)))
        {
            out.push(member_at(sym)?);
        }
        let Some(children) = sym.get("children") else {
            continue;
        };
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
