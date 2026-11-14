/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::across_projects::{names_word, read_name_scan_text};
pub(crate) use super::declared_at::declared_at;
use super::declared_at::{code_declarations, is_cargo_target_root};
use super::nested_projects::{MAX_SCANNED_FILES, source_files};
use super::types::SymbolHit;
use crate::tools::execute_lsp_query;
use std::net::SocketAddr;
use std::path::Path;
use url::Url;

#[allow(unused_imports)]
pub(crate) use super::bare_members::{
    UNINDEXED_MEMBERS_BUDGET, collect_unqualified_members, unindexed_members,
};

/// The most declarations of a name the index lacks that an answer names.
pub(crate) const MAX_UNINDEXED_DECLARATIONS: usize = 3;

/// How long unindexed declaration scanning can spend before returning.
pub(crate) const UNINDEXED_DECLARATION_BUDGET: std::time::Duration =
    std::time::Duration::from_secs(10);

fn is_declaration_scan_document(path: &Path) -> bool {
    matches!(
        crate::sync::engine_for_file(path),
        Some("markdown" | "toml" | "json" | "jsonc" | "yaml")
    )
}

fn prioritized_source_files(
    root: &Path,
    include_rust: bool,
    limit: usize,
) -> Vec<std::path::PathBuf> {
    let eligible = |path: &std::path::PathBuf| {
        crate::sync::engine_for_file(path).is_some_and(|engine| include_rust || engine != "rust")
    };
    let mut source_candidates = source_files(root)
        .filter(|path| eligible(path) && !is_declaration_scan_document(path))
        .take(limit)
        .collect::<Vec<_>>();
    let remaining = limit.saturating_sub(source_candidates.len());
    source_candidates.extend(
        source_files(root)
            .filter(|path| eligible(path) && is_declaration_scan_document(path))
            .take(remaining),
    );
    source_candidates
}

pub(super) fn collect_candidate_files(
    root: &Path,
    hint: Option<&Path>,
    include_rust: bool,
) -> Vec<std::path::PathBuf> {
    let mut candidate_files = Vec::new();
    if let Some(h) = hint {
        let p = if h.is_absolute() {
            h.to_path_buf()
        } else {
            root.join(h)
        };
        if p.is_file() {
            if include_rust || crate::sync::engine_for_file(&p) != Some("rust") {
                candidate_files.push(p);
            }
        } else if p.is_dir() {
            candidate_files.extend(prioritized_source_files(&p, include_rust, 32));
        }
    }
    for path in prioritized_source_files(root, include_rust, MAX_SCANNED_FILES) {
        if !candidate_files.contains(&path) {
            candidate_files.push(path);
        }
    }
    candidate_files
}

pub(crate) async fn find_unindexed_declarations(
    root: &Path,
    name: &str,
    hint: Option<&Path>,
) -> Vec<SymbolHit> {
    if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return Vec::new();
    }
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(4);
    let mut hits = Vec::new();
    for path in collect_candidate_files(root, hint, false) {
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        let Some(text) = read_name_scan_text(&path) else {
            continue;
        };
        if !names_word(&text, name) {
            continue;
        }
        for (index, col) in code_declarations(&text, &path, name) {
            hits.push(SymbolHit {
                path: path.clone(),
                name: name.to_string(),
                kind: "Declaration",
                container: None,
                line: index as u32 + 1,
                col: col as u32 + 1,
            });
            if hits.len() >= 10 {
                return hits;
            }
        }
    }
    hits
}

/// Resolves declarations of `name` in source files (including Rust) that the language server loads
/// (`!hover.is_null()` at the declaration site) when `workspace/symbol` omits them (#1125).
pub(crate) async fn find_loaded_unindexed_declarations(
    remote: SocketAddr,
    root: &Path,
    name: &str,
    hint: Option<&Path>,
) -> Vec<SymbolHit> {
    if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return Vec::new();
    }
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(4);
    let mut hits = Vec::new();
    for path in collect_candidate_files(root, hint, true) {
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        let Some(text) = read_name_scan_text(&path) else {
            continue;
        };
        if !names_word(&text, name) {
            continue;
        }
        for (index, col) in code_declarations(&text, &path, name) {
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            let Ok(uri) = Url::from_file_path(&path) else {
                continue;
            };
            let hover_fut = execute_lsp_query(
                remote,
                root,
                &path,
                "textDocument/hover",
                serde_json::json!({
                    "textDocument": { "uri": uri.to_string() },
                    "position": { "line": index, "character": col }
                }),
            );
            let hover = match tokio::time::timeout_at(deadline, hover_fut).await {
                Ok(Ok(val)) => val,
                _ => serde_json::Value::Null,
            };
            if !hover.is_null() {
                hits.push(SymbolHit {
                    path: path.clone(),
                    name: name.to_string(),
                    kind: "Declaration",
                    container: None,
                    line: index as u32 + 1,
                    col: col as u32 + 1,
                });
                if hits.len() >= MAX_UNINDEXED_DECLARATIONS {
                    return hits;
                }
            }
        }
    }
    hits
}

/// Where the checkout's own source files declare `name` when the index has no symbol by that
/// name, and why the analyzer has nothing there: a file no target includes (it has no hover at
/// the declaration), or an item the index does not list (#379). Empty when no file declares it.
pub(crate) async fn unindexed_declarations(remote: SocketAddr, root: &Path, name: &str) -> String {
    if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return String::new();
    }
    let deadline = tokio::time::Instant::now() + UNINDEXED_DECLARATION_BUDGET;
    let mut out = String::new();
    let mut listed = 0usize;
    let files = source_files(root)
        .filter(|path| crate::sync::engine_for_file(path).is_some())
        .take(MAX_SCANNED_FILES);
    for path in files {
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        let Some(text) = read_name_scan_text(&path) else {
            continue;
        };
        if !names_word(&text, name) {
            continue;
        }
        let lines: Vec<&str> = text.lines().collect();
        for (index, col) in code_declarations(&text, &path, name) {
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            let line = lines[index];
            let Ok(uri) = Url::from_file_path(&path) else {
                continue;
            };
            let hover_fut = execute_lsp_query(
                remote,
                root,
                &path,
                "textDocument/hover",
                serde_json::json!({
                    "textDocument": { "uri": uri.to_string() },
                    "position": { "line": index, "character": col }
                }),
            );
            let hover = match tokio::time::timeout_at(deadline, hover_fut).await {
                Ok(Ok(val)) => val,
                Ok(Err(_)) => serde_json::Value::Null,
                Err(_) => {
                    tracing::warn!("unindexed declaration hover query timed out against budget");
                    return out;
                }
            };

            let why = if hover.is_null() {
                if is_cargo_target_root(&path) {
                    "in a Cargo target root that the analyzer has not loaded into its crate graph \
                     yet"
                } else {
                    "in a file the analyzer does not load: no target includes it (for Rust, no \
                     `mod` chain from a crate root reaches it)"
                }
            } else if path.extension().is_some_and(|e| e == "swift") {
                // sourcekit-lsp answers `workspace/symbol` from a build index only; an
                // extension's members are not in a function body (#2014).
                "which the analyzer sees but lists no workspace symbols for until the package \
                 has a build index: ask by its position or pass its file as `path`"
            } else {
                "which the analyzer sees but its index does not list (an item inside a function \
                 body is not indexed): ask by its position"
            };
            out.push_str(&format!(
                "\n`{name}` is declared at {}:{}:{} (`{}`), {why}.",
                path.strip_prefix(root).unwrap_or(&path).display(),
                index + 1,
                col + 1,
                line.trim()
            ));
            listed += 1;
            if listed == MAX_UNINDEXED_DECLARATIONS {
                return out;
            }
        }
    }
    out
}
