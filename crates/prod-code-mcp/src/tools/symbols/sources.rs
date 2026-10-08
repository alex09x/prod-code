/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::SymbolHit;
use std::net::SocketAddr;
use std::path::Path;

/// Texts of files a symbol hit points at that are not on this machine, keyed by path.
pub(crate) type RemoteSources = std::collections::HashMap<std::path::PathBuf, String>;

/// The most dependency files read from the gateway for one resolution.
pub(crate) const MAX_REMOTE_SOURCES: usize = 16;

/// Reads from the gateway the files of `hits` that do not exist here. A dependency crate's
/// source lives only in the build node's cargo registry, and the checks that tell a definition
/// from its re-export read the file (#271).
pub(crate) async fn remote_sources(remote: SocketAddr, hits: &[SymbolHit]) -> RemoteSources {
    let mut texts = RemoteSources::new();
    for hit in hits {
        if texts.len() >= MAX_REMOTE_SOURCES {
            break;
        }
        if hit.path.exists() || texts.contains_key(&hit.path) {
            continue;
        }
        if let Ok((bytes, _)) =
            crate::remote_fs::read_remote_file(remote, &hit.path.to_string_lossy(), 4 << 20).await
        {
            texts.insert(
                hit.path.clone(),
                String::from_utf8_lossy(&bytes).into_owned(),
            );
        }
    }
    texts
}

/// The text of `path`: from `remote` when it was read from the gateway, otherwise from disk.
pub(crate) fn source_text<'a>(
    path: &Path,
    remote: &'a RemoteSources,
) -> Option<std::borrow::Cow<'a, str>> {
    match remote.get(path) {
        Some(text) => Some(std::borrow::Cow::Borrowed(text.as_str())),
        None => std::fs::read_to_string(path)
            .ok()
            .map(std::borrow::Cow::Owned),
    }
}

/// Whether the symbol on 1-based `line` of `path` is an `extension` of a type (Swift), which an
/// index lists under the type's name next to the type's declaration (#358).
pub(crate) fn is_extension_declaration(path: &Path, remote: &RemoteSources, line: u32) -> bool {
    let Some(text) = source_text(path, remote) else {
        return false;
    };
    let Some(target) = (line as usize).checked_sub(1) else {
        return false;
    };
    text.lines().nth(target).is_some_and(|l| {
        l.split_whitespace().find(|w| {
            !w.starts_with('@')
                && !matches!(
                    *w,
                    "public" | "private" | "fileprivate" | "internal" | "open" | "package"
                )
        }) == Some("extension")
    })
}

pub(crate) fn is_use_declaration(path: &Path, remote: &RemoteSources, line: u32) -> bool {
    let Some(text) = source_text(path, remote) else {
        return false;
    };
    let lines: Vec<&str> = text.lines().collect();
    let Some(target) = (line as usize).checked_sub(1) else {
        return false;
    };
    // Every `use` runs from its first line to the line with its `;`.
    let mut at = 0;
    while at <= target && at < lines.len() {
        if !starts_use(lines[at]) {
            at += 1;
            continue;
        }
        let end = (at..lines.len())
            .find(|n| lines[*n].contains(';'))
            .unwrap_or(at);
        if (at..=end).contains(&target) {
            return true;
        }
        at = end + 1;
    }
    false
}

/// Does this line start a `use` declaration, with or without a visibility?
pub(crate) fn starts_use(row: &str) -> bool {
    let row = row.trim_start();
    let row = match row.strip_prefix("pub") {
        Some(rest) if rest.starts_with('(') => rest
            .find(')')
            .map_or(rest, |close| &rest[close + 1..])
            .trim_start(),
        Some(rest) if rest.starts_with(char::is_whitespace) => rest.trim_start(),
        _ => row,
    };
    row.starts_with("use ")
}

/// Finds the byte offset of `bare` as an identifier token on `row`, checking word boundaries (#887, #888).
pub fn find_identifier_on_line(row: &str, bare: &str) -> Option<usize> {
    let is_ident = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    for (idx, _) in row.match_indices(bare) {
        let before_ok = idx == 0 || !row[..idx].chars().next_back().is_some_and(is_ident);
        let after_idx = idx + bare.len();
        let after_ok =
            after_idx >= row.len() || !row[after_idx..].chars().next().is_some_and(is_ident);
        if before_ok && after_ok {
            return Some(idx);
        }
    }
    None
}

/// Whether `name` is the identifier at the 1-based line/column of `path` (false when the
/// file cannot be read).
pub(crate) fn identifier_at(
    path: &Path,
    remote: &RemoteSources,
    line: u32,
    col: u32,
    name: &str,
) -> bool {
    let Some(text) = source_text(path, remote) else {
        return false;
    };
    let Some(row) = text.lines().nth(line.saturating_sub(1) as usize) else {
        return false;
    };
    let start = row
        .char_indices()
        .nth(col.saturating_sub(1) as usize)
        .map(|(i, _)| i)
        .unwrap_or(row.len());
    let bare = name.split(['(', '<']).next().unwrap_or(name);
    if row[start..].starts_with(bare) {
        return true;
    }
    find_identifier_on_line(row, bare).is_some()
}

/// The files a workspace edit rewrites, as (path, whole new content). The gateway answers a
/// structural rewrite with `documentChanges`, one whole-file replacement per file, so the
/// caller can diff each against what is on disk.
pub(crate) fn rewritten_files(edit: &serde_json::Value) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for change in edit
        .get("documentChanges")
        .and_then(|c| c.as_array())
        .map(|a| a.as_slice())
        .unwrap_or_default()
    {
        let Some(uri) = change
            .get("textDocument")
            .and_then(|t| t.get("uri"))
            .and_then(|u| u.as_str())
        else {
            continue;
        };
        let Some(new_text) = change
            .get("edits")
            .and_then(|e| e.as_array())
            .and_then(|e| e.first())
            .and_then(|e| e.get("newText"))
            .and_then(|t| t.as_str())
        else {
            continue;
        };
        out.push((crate::remote_fs::uri_to_path(uri), new_text.to_string()));
    }
    out
}
