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
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use crate::signature::parse::param_span;
use crate::signature::types::Reference;

pub fn locate_declaration(text: &str, name: &str, old_inner: &str) -> Option<(usize, usize)> {
    let needle = format!("fn {name}");
    let mut found = None;
    let mut from = 0;
    while let Some(i) = text[from..].find(&needle) {
        let at = from + i + 3; // the name, which is what `param_span` starts from
        from = at;
        // A longer name that starts with this one is not this function.
        let after = text[at + name.len()..].chars().next();
        if after.is_some_and(|c| c.is_alphanumeric() || c == '_') {
            continue;
        }
        let Some((_, open, close)) = param_span(text, at) else {
            continue;
        };
        if text[open..close] == *old_inner {
            if found.is_some() {
                return None;
            }
            found = Some((open, close));
        }
    }
    found
}

/// A workspace edit that replaces each file wholesale, the shape the gateway answers a
/// structural rewrite with.
pub fn whole_file_edit(files: &BTreeMap<PathBuf, String>) -> serde_json::Value {
    let changes: Vec<serde_json::Value> = files
        .iter()
        .map(|(path, new_text)| {
            let old_lines = std::fs::read_to_string(path)
                .map(|t| t.lines().count())
                .unwrap_or(0);
            serde_json::json!({
                "textDocument": { "uri": prod_code_protocol::path::file_uri(path), "version": null },
                "edits": [ {
                    "range": {
                        "start": { "line": 0, "character": 0 },
                        "end": { "line": old_lines, "character": 0 }
                    },
                    "newText": new_text
                } ]
            })
        })
        .collect();
    serde_json::json!({ "documentChanges": changes })
}

/// Runs a structural rewrite over the whole workspace, resolved in `context`'s own scope.
///
/// The position is deliberately (0, 0): the engine then resolves the rule in the body of that
/// file's first function, which is the module the declaration lives in, so the bare name
/// resolves. A position on the declaration itself is an item position, where it may not.
pub async fn structural_replace(
    remote: SocketAddr,
    root: &Path,
    context: &Path,
    rule: &str,
) -> Result<serde_json::Value> {
    let uri = url::Url::from_file_path(context)
        .map_err(|_| anyhow::anyhow!("invalid path {:?}", context))?
        .to_string();
    crate::tools::execute_lsp_query(
        remote,
        root,
        context,
        "prodCode/structuralReplace",
        serde_json::json!({
            "rule": rule,
            "scope": serde_json::Value::Null,
            "textDocument": { "uri": uri },
            "position": { "line": 0, "character": 0 },
        }),
    )
    .await
}

/// Every reference to the symbol at `file:line:col`, as (file, line, column), without the
/// declaration itself.
pub async fn references(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
) -> Result<Vec<Reference>> {
    let uri = url::Url::from_file_path(file)
        .map_err(|_| anyhow::anyhow!("invalid path {:?}", file))?
        .to_string();
    let params = serde_json::json!({
        "textDocument": { "uri": uri },
        "position": { "line": line.saturating_sub(1), "character": col.saturating_sub(1) },
        "context": { "includeDeclaration": false },
    });
    let ask = || {
        crate::tools::execute_lsp_query(
            remote,
            root,
            file,
            "textDocument/references",
            params.clone(),
        )
    };
    let mut res = ask().await?;
    // A language server still reading the project answers with no references, and a signature
    // refactoring would take that for "no callers" and leave every call site behind (#284).
    // rust-analyzer answers from a database that is already loaded.
    if crate::sync::engine_for_file(file) != Some("rust") {
        for _ in 0..crate::impact::COLD_RETRIES {
            if res.as_array().is_some_and(|refs| !refs.is_empty()) {
                break;
            }
            tokio::time::sleep(crate::impact::COLD_WAIT).await;
            res = ask().await?;
        }
    }
    parse_locations(&res)
}

/// The locations of a `textDocument/references` answer, 1-based. `null` is the protocol's "none";
/// any other answer that is not a list of locations, and any entry without a file or a start, is
/// an error: a planner that dropped it would rewrite every call site but that one.
pub fn parse_locations(res: &serde_json::Value) -> Result<Vec<Reference>> {
    if res.is_null() {
        return Ok(Vec::new());
    }
    let entries = res
        .as_array()
        .with_context(|| format!("the analyzer's references are not a list: {}", brief(res)))?;
    let mut out = Vec::with_capacity(entries.len());
    for (n, loc) in entries.iter().enumerate() {
        let position = |key: &str| {
            loc.pointer(&format!("/range/start/{key}"))
                .and_then(|v| v.as_u64())
                .and_then(|v| u32::try_from(v).ok())
        };
        let (Some(uri), Some(l), Some(c)) = (
            loc.get("uri").and_then(|u| u.as_str()),
            position("line"),
            position("character"),
        ) else {
            anyhow::bail!(
                "reference {} of {} from the analyzer has no file or start position: {}",
                n + 1,
                entries.len(),
                brief(loc)
            );
        };
        // 1-based, and a position one past the last `u32` is in no file: an error, not a wrap.
        let (Some(line), Some(col)) = (l.checked_add(1), c.checked_add(1)) else {
            anyhow::bail!(
                "reference {} of {} from the analyzer is at line {l}, character {c} (0-based), \
                 which no file has: {}",
                n + 1,
                entries.len(),
                brief(loc)
            );
        };
        let path = local_file(uri).with_context(|| {
            format!(
                "reference {} of {} from the analyzer, `{uri}`, is not a local file URI",
                n + 1,
                entries.len()
            )
        })?;
        out.push((path, line, col));
    }
    Ok(out)
}

/// The path of a `file:` URI with no host other than `localhost`, percent-decoded; `None` for
/// any other scheme, a remote host, or a relative path. A reference the change cannot open is
/// a call site it can neither check nor rewrite.
pub fn local_file(uri: &str) -> Option<PathBuf> {
    let url = url::Url::parse(uri).ok()?;
    if url.scheme() != "file" {
        return None;
    }
    let path = url.to_file_path().ok()?;
    path.is_absolute().then_some(path)
}

/// An answer, cut short for an error message.
pub fn brief(value: &serde_json::Value) -> String {
    let text = value.to_string();
    match text.char_indices().nth(200) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text,
    }
}

/// How many files [`unreported_callers`] names at most: each one is opened and checked with the
/// rewritten ones.
pub const UNREPORTED_LIMIT: usize = 20;

/// The files of `file`'s language under `root`, other than `file` and `checked`, that write
/// `name(`: where a caller can be that the analyzer did not report. A language server may answer
/// `references` from an index that is not there yet (sourcekit-lsp reads the one a build writes,
/// and finds nothing in a package never built, #294), and a caller a refactoring did not rewrite
/// then breaks unseen unless its file is checked together with the rewritten ones. A file that
/// only has another function of the same name costs a check and changes nothing. Rust is not
/// searched: rust-analyzer answers from the crate graph it has loaded.
pub fn unreported_callers(
    root: &Path,
    file: &Path,
    name: &str,
    checked: &[PathBuf],
) -> Vec<PathBuf> {
    let family = |p: &Path| match crate::lang::language_id_for_path(p) {
        "c" | "cpp" | "objective-c" | "objective-cpp" => "c",
        "typescript" | "typescriptreact" | "javascript" | "javascriptreact" => "javascript",
        other => other,
    };
    let wanted = family(file);
    if name.is_empty() || matches!(wanted, "rust" | "plaintext") {
        return Vec::new();
    }
    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let base = canonical(root);
    let known: Vec<PathBuf> = checked
        .iter()
        .map(|p| canonical(p))
        .chain([canonical(file)])
        .collect();
    let mut found = Vec::new();
    for entry in ignore::WalkBuilder::new(root)
        .max_filesize(Some(1 << 20))
        .build()
        .flatten()
    {
        let path = entry.path();
        if !entry.file_type().is_some_and(|t| t.is_file()) || family(path) != wanted {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let path = canonical(path);
        if !writes_call(&text, name) || known.contains(&path) {
            continue;
        }
        // Spelled under `root` as the caller gave it, so that it is translated like the others.
        found.push(match path.strip_prefix(&base) {
            Ok(rel) => root.join(rel),
            Err(_) => path,
        });
        if found.len() == UNREPORTED_LIMIT {
            break;
        }
    }
    found.sort();
    found
}

/// Whether `text` writes `name(` with `name` as a whole word, a call or a declaration.
pub fn writes_call(text: &str, name: &str) -> bool {
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    text.match_indices(name).any(|(at, _)| {
        !text[..at].chars().next_back().is_some_and(ident)
            && text[at + name.len()..].trim_start().starts_with('(')
    })
}
