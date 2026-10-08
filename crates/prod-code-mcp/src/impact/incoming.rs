/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::BTreeMap;
use std::path::Path;
use url::Url;

use crate::session::LspSession;

use super::rust_attr::rust_test_marker;
use super::symbols::one_based;
use super::test_cmd::{file_language, looks_like_test, rel, test_marker};
use super::types::{Gap, Reach, Symbol};

/// Adds `gap` unless it is already noted: two walks through one function meet the same gap.
pub(crate) fn note(gaps: &mut Vec<Gap>, gap: Gap) {
    if !gaps.contains(&gap) {
        gaps.push(gap);
    }
}

/// Says that `method` answered with something other than the shape the protocol gives it,
/// showing the start of the answer.
pub fn unreadable(method: &str, answer: &serde_json::Value) -> String {
    let text = answer.to_string();
    let shown: String = text.chars().take(80).collect();
    let cut = if shown.len() < text.len() { "…" } else { "" };
    format!("{method} answered with something it cannot read: {shown}{cut}")
}

/// The name a test runner selects the function `name` declared at `line` of `file` by, when it
/// is a test: flagged by the analyzer, named or placed like one, or marked as one in its source.
/// Rust deliberately accepts only a source attribute: an analyzer flag can describe a helper in
/// test context that is not itself a libtest entry. An unreadable or structurally unclassifiable
/// Rust declaration is a gap, not evidence that no test reaches the change.
pub(crate) fn test_name(
    root: &Path,
    language: &str,
    name: &str,
    file: &str,
    line: u32,
    flagged: bool,
) -> std::result::Result<Option<String>, String> {
    if language == "rust" {
        let text = std::fs::read_to_string(root.join(file)).map_err(|e| {
            format!(
                "the Rust source needed to classify {name} as a runnable test cannot be read: {e}"
            )
        })?;
        return rust_test_marker(&text, line, name).map(|marked| marked.map(|_| name.to_string()));
    }
    if flagged {
        return Ok(Some(name.to_string()));
    }
    if looks_like_test(language, name, file) {
        return Ok(Some(name.to_string()));
    }
    // Beyond names: an attribute, a registration macro, a `TestCase` class (#201).
    let text = match std::fs::read_to_string(root.join(file)) {
        Ok(text) => text,
        Err(_) => return Ok(None),
    };
    Ok(test_marker(language, &text, line, name))
}

/// Times an empty answer from a managed language server is asked again, and the wait before
/// each: a server still reading the project answers empty (#202, #284).

/// What the analyzer said about the callers of one function.
pub(crate) enum Incoming {
    /// The functions that call it, each with whether it is a test; possibly none.
    Callers(Vec<(Symbol, bool)>),
    /// It has no call-hierarchy item at the function's name.
    NoItem,
    /// A request failed, or its answer is not the shape the protocol gives it.
    Failed(String),
}

/// The functions that call `sym`, each with whether it is a test (the analyzer's `isTest`, the
/// language's naming conventions, or a marker in the source).
pub(crate) async fn incoming_calls(
    session: &mut LspSession,
    root: &Path,
    language: &str,
    sym: &Symbol,
    deadline: Option<tokio::time::Instant>,
) -> Incoming {
    let abs = root.join(&sym.file);
    let Ok(uri) = Url::from_file_path(&abs).map(|u| u.to_string()) else {
        return Incoming::Failed(format!("{} is not a file path", sym.file));
    };
    let position = serde_json::json!({ "line": sym.line.saturating_sub(1), "character": sym.col.saturating_sub(1) });
    let prepare = "textDocument/prepareCallHierarchy";
    let query_timeout = match deadline {
        Some(dl) => {
            let now = tokio::time::Instant::now();
            if now >= dl {
                return Incoming::Failed("impact BFS deadline expired".to_string());
            }
            std::cmp::min(std::time::Duration::from_secs(30), dl - now)
        }
        None => std::time::Duration::from_secs(30),
    };
    let items = match tokio::time::timeout(
        query_timeout,
        session.query(
            &abs,
            prepare,
            serde_json::json!({ "textDocument": { "uri": uri }, "position": position }),
        ),
    )
    .await
    {
        Ok(Ok(items)) => items,
        Ok(Err(e)) => return Incoming::Failed(format!("{e:#}")),
        Err(_) => return Incoming::Failed("prepareCallHierarchy timed out".to_string()),
    };
    let items = match items {
        serde_json::Value::Null => return Incoming::NoItem,
        serde_json::Value::Array(all) if all.is_empty() => return Incoming::NoItem,
        serde_json::Value::Array(all)
            if all.iter().all(|item| {
                item.get("name")
                    .and_then(|n| n.as_str())
                    .is_some_and(|n| !n.is_empty())
            }) =>
        {
            all
        }
        other => return Incoming::Failed(unreadable(prepare, &other)),
    };
    // One name can stand for several items (a declaration and its definition, overloads):
    // the callers of each are callers of the function.
    let method = "callHierarchy/incomingCalls";
    let mut out: Vec<(Symbol, bool)> = Vec::new();
    for item in items {
        let per_item_timeout = match deadline {
            Some(dl) => {
                let now = tokio::time::Instant::now();
                if now >= dl {
                    break;
                }
                std::cmp::min(std::time::Duration::from_secs(30), dl - now)
            }
            None => std::time::Duration::from_secs(30),
        };
        let incoming = match tokio::time::timeout(
            per_item_timeout,
            session.query(&abs, method, serde_json::json!({ "item": item })),
        )
        .await
        {
            Ok(Ok(incoming)) => incoming,
            Ok(Err(e)) => return Incoming::Failed(format!("{e:#}")),
            Err(_) => return Incoming::Failed("incomingCalls timed out".to_string()),
        };
        let edges = match incoming {
            // The protocol's "no calls", after an item was found.
            serde_json::Value::Null => continue,
            serde_json::Value::Array(edges) => edges,
            other => return Incoming::Failed(unreadable(method, &other)),
        };
        for edge in &edges {
            match caller(root, language, edge) {
                Ok(Some(found)) if !out.contains(&found) => out.push(found),
                Ok(_) => {}
                Err(error) => return Incoming::Failed(error),
            }
        }
    }
    Incoming::Callers(out)
}

/// The caller an incoming call names, with whether it is a test; `None` when it lies outside
/// the checkout. A call without its caller's name or place, or with a position that is not a
/// line and column, is an error: skipped, it would read as "no caller".
pub(crate) fn caller(
    root: &Path,
    language: &str,
    edge: &serde_json::Value,
) -> std::result::Result<Option<(Symbol, bool)>, String> {
    let malformed = || unreadable("callHierarchy/incomingCalls", edge);
    let from = edge
        .get("from")
        .filter(|f| f.is_object())
        .ok_or_else(malformed)?;
    let uri = from
        .get("uri")
        .and_then(|u| u.as_str())
        .ok_or_else(malformed)?;
    let name = from
        .get("name")
        .and_then(|n| n.as_str())
        .filter(|n| !n.is_empty())
        .ok_or_else(malformed)?;
    let start = from
        .get("selectionRange")
        .or_else(|| from.get("range"))
        .and_then(|r| r.get("start"))
        .ok_or_else(malformed)?;
    let (line, col) = one_based(start, "line")
        .zip(one_based(start, "character"))
        .ok_or_else(malformed)?;
    let flagged = match edge.get("isTest") {
        None | Some(serde_json::Value::Null) => false,
        Some(serde_json::Value::Bool(flag)) => *flag,
        Some(_) => return Err(malformed()),
    };
    let absolute = Url::parse(uri)
        .ok()
        .and_then(|url| url.to_file_path().ok())
        .ok_or_else(malformed)?;
    if !absolute.is_absolute() {
        return Err(malformed());
    }
    let file = rel(root, uri);
    if file.starts_with('/') {
        return Ok(None); // outside the checkout
    }
    // Module-level code (a test file's top-level `it(...)` calls) is reported with the file as
    // its name: keep it checkout-relative.
    let mut name = if name.starts_with('/') {
        rel(root, name)
    } else {
        name.to_string()
    };
    let caller_file_lang = file_language(root, &file).unwrap_or(language);
    let is_test = match test_name(root, caller_file_lang, &name, &file, line, flagged)? {
        Some(test) => {
            name = test;
            true
        }
        None => false,
    };
    Ok(Some((
        Symbol {
            name,
            file,
            line,
            col,
        },
        is_test,
    )))
}

/// The changed functions that reach the failing test `name` (`tests::doubles`,
/// `MathTests.testAdds()`, `tests/test_x.py::test_y`), nearest first, each once.
pub fn suspects_for(reaches: &[Reach], name: &str) -> Vec<(Symbol, usize)> {
    let bare = |n: &str| -> String {
        n.rsplit("::")
            .next()
            .unwrap_or(n)
            .rsplit('.')
            .next()
            .unwrap_or(n)
            .trim_end_matches("()")
            .to_string()
    };
    let wanted = bare(name);
    let mut best: BTreeMap<Symbol, usize> = BTreeMap::new();
    for r in reaches.iter().filter(|r| bare(&r.test.name) == wanted) {
        let hops = best.entry(r.changed.clone()).or_insert(r.hops);
        *hops = (*hops).min(r.hops);
    }
    let mut out: Vec<(Symbol, usize)> = best.into_iter().collect();
    out.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
    out
}
