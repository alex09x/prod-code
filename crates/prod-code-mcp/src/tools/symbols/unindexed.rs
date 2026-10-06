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
use super::matching::{bare_symbol_name, owner_segments};
use super::nested_projects::{MAX_SCANNED_FILES, source_files};
use super::type_members::member_at;
use super::types::{SymbolHit, symbol_kind_name};
use crate::tools::execute_lsp_query;
use anyhow::Result;
use std::net::SocketAddr;
use std::path::Path;
use url::Url;

pub(crate) const UNINDEXED_MEMBERS_BUDGET: std::time::Duration = std::time::Duration::from_secs(10);

/// Where the checkout's source files declare `member` as a struct/class/interface field or
/// method when the language server's `workspace/symbol` index omitted it (e.g. rust-analyzer
/// does not index struct fields).
pub(crate) async fn unindexed_members(
    remote: SocketAddr,
    root: &Path,
    member: &str,
    hint: Option<&Path>,
) -> Result<Vec<SymbolHit>> {
    let deadline = tokio::time::Instant::now() + UNINDEXED_MEMBERS_BUDGET;
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
    for path in source_files(root)
        .filter(|p| crate::sync::engine_for_file(p).is_some())
        .take(MAX_SCANNED_FILES)
    {
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        if !files.contains(&path)
            && std::fs::read_to_string(&path).is_ok_and(|text| names_word(&text, member))
        {
            files.push(path);
            if files.len() >= 8 {
                break;
            }
        }
    }

    let mut members: Vec<SymbolHit> = Vec::new();
    for file in &files {
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
        collect_unqualified_members(&outline, file, member, &[], &mut members)?;
    }
    Ok(members)
}

pub(crate) fn collect_unqualified_members(
    symbols: &serde_json::Value,
    path: &Path,
    member: &str,
    ancestors: &[String],
    out: &mut Vec<SymbolHit>,
) -> Result<()> {
    let is_member = |sym: &serde_json::Value| {
        let name = sym.get("name").and_then(|n| n.as_str()).unwrap_or("");
        bare_symbol_name(name).eq_ignore_ascii_case(member)
    };
    for sym in symbols.as_array().into_iter().flatten() {
        let name = sym.get("name").and_then(|n| n.as_str()).unwrap_or("");
        let kind = sym.get("kind").and_then(|k| k.as_u64()).unwrap_or(0);
        let mut declared = ancestors.to_vec();
        declared.extend(owner_segments(name));

        if is_member(sym) && (!ancestors.is_empty() || matches!(kind, 6..=9 | 22)) {
            let (m_name, m_kind, line, col) = member_at(sym)?;
            let container = ancestors.last().cloned().or_else(|| {
                sym.get("containerName")
                    .and_then(|c| c.as_str())
                    .map(str::to_string)
            });
            let hit = SymbolHit {
                path: path.to_path_buf(),
                name: m_name,
                kind: symbol_kind_name(m_kind),
                container,
                line,
                col,
            };
            if !out
                .iter()
                .any(|m| m.path == hit.path && m.line == hit.line && m.col == hit.col)
            {
                out.push(hit);
            }
        }

        if let Some(children) = sym.get("children") {
            let nested_ancestors = if matches!(kind, 2..=5 | 10 | 11 | 23) {
                declared
            } else {
                ancestors.to_vec()
            };
            collect_unqualified_members(children, path, member, &nested_ancestors, out)?;
        }
    }
    Ok(())
}

/// The most declarations of a name the index lacks that an answer names.
pub(crate) const MAX_UNINDEXED_DECLARATIONS: usize = 3;

/// How long unindexed declaration scanning can spend before returning.
pub(crate) const UNINDEXED_DECLARATION_BUDGET: std::time::Duration =
    std::time::Duration::from_secs(10);

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
    let mut files = Vec::new();
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
                if std::fs::read_to_string(&entry).is_ok_and(|text| names_word(&text, name)) {
                    files.push(entry);
                }
            }
        }
    }
    for path in source_files(root)
        .filter(|path| crate::sync::engine_for_file(path).is_some())
        .take(MAX_SCANNED_FILES)
    {
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        if !files.contains(&path)
            && std::fs::read_to_string(&path).is_ok_and(|text| names_word(&text, name))
        {
            files.push(path);
            if files.len() >= 16 {
                break;
            }
        }
    }

    for path in files {
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        let Some(text) = read_name_scan_text(&path) else {
            continue;
        };
        for (index, line) in text.lines().enumerate() {
            if let Some(col) = declared_at(line, name) {
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
        for (index, line) in text.lines().enumerate() {
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            let Some(col) = declared_at(line, name) else {
                continue;
            };
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
                "in a file the analyzer does not load: no target includes it (for Rust, no `mod` \
                 chain from a crate root reaches it)"
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

/// The 0-based column where `line` declares `name`: the name right after a declaring keyword
/// (`fn`, `struct`, `func`, `class`, `def`, ...) or after a Go method's receiver
/// (`func (s *T) name`). `None` for a line that only uses it.
pub(crate) fn declared_at(line: &str, name: &str) -> Option<usize> {
    const KEYWORDS: &[&str] = &[
        "fn",
        "struct",
        "enum",
        "trait",
        "type",
        "union",
        "mod",
        "const",
        "static",
        "macro_rules!",
        "func",
        "function",
        "class",
        "interface",
        "def",
        "protocol",
        "actor",
        "var",
        "let",
        "val",
    ];
    let chars: Vec<char> = line.chars().collect();
    let wanted: Vec<char> = name.chars().collect();
    let is_name = |c: Option<&char>| c.is_some_and(|c| c.is_alphanumeric() || *c == '_');
    (0..chars.len()).find(|&col| {
        if !chars[col..].starts_with(&wanted)
            || (col > 0 && is_name(chars.get(col - 1)))
            || is_name(chars.get(col + wanted.len()))
        {
            return false;
        }
        let before: String = chars[..col].iter().collect();
        let trimmed = before.trim_end();
        if trimmed.len() == before.len() {
            return false;
        }
        let last = trimmed
            .rsplit(|c: char| c.is_whitespace() || c == '(')
            .next()
            .unwrap_or("");
        if KEYWORDS.contains(&last)
            || (trimmed.ends_with(')') && trimmed.trim_start().starts_with("func"))
        {
            return true;
        }
        let after: Vec<char> = chars[col + wanted.len()..]
            .iter()
            .copied()
            .skip_while(|c| c.is_whitespace())
            .collect();
        if after.first() == Some(&':') && after.get(1) != Some(&':') {
            let before_trimmed = trimmed.trim_start();
            if before_trimmed.is_empty()
                || before_trimmed == "pub"
                || before_trimmed.starts_with("pub(")
                || before_trimmed == "mut"
                || before_trimmed == "val"
                || before_trimmed == "var"
                || before_trimmed == "let"
                || before_trimmed == "public"
                || before_trimmed == "private"
                || before_trimmed == "protected"
                || before_trimmed == "readonly"
            {
                return true;
            }
        }
        false
    })
}
