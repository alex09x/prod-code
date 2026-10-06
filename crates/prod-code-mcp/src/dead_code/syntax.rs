/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::CandidateSymbol;
use anyhow::Result;

/// How many references a `textDocument/references` answer lists, or why it says nothing about
/// them. Only a list, empty or not, is an answer; `null` is the protocol's "no result", which
/// does not tell an unreferenced symbol from one the analyzer did not search for.
pub fn reference_count(answer: Result<serde_json::Value>) -> std::result::Result<usize, String> {
    match answer {
        Ok(serde_json::Value::Array(found)) => Ok(found.len()),
        Ok(serde_json::Value::Null) => Err(
            "textDocument/references answered null, which does not say whether anything references it"
                .to_string(),
        ),
        Ok(other) => Err(crate::impact::unreadable("textDocument/references", &other)),
        Err(e) => Err(format!("{e:#}")),
    }
}

pub(crate) fn kind_name(kind: u64) -> Option<&'static str> {
    Some(match kind {
        5 => "class",
        6 => "method",
        10 => "enum",
        11 => "interface",
        12 => "function",
        23 => "struct",
        _ => return None,
    })
}

/// Whether a source line declares an exported / public item in `language`.
pub fn is_exported(language: &str, name: &str, line: &str) -> bool {
    let t = line.trim_start();
    match language {
        "rust" => t.starts_with("pub ") || t.starts_with("pub("),
        "go" => name.chars().next().is_some_and(|c| c.is_ascii_uppercase()),
        "typescript" => t.starts_with("export "),
        "swift" => t.starts_with("public ") || t.starts_with("open "),
        "cpp" => false,
        "python" => !name.starts_with('_'),
        _ => false,
    }
}

pub(crate) fn is_test_path(language: &str, rel: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    lower.contains("/tests/")
        || lower.starts_with("tests/")
        || lower.contains("/test/")
        || match language {
            "go" => lower.ends_with("_test.go"),
            "python" => lower
                .rsplit('/')
                .next()
                .is_some_and(|b| b.starts_with("test_") || b.ends_with("_test.py")),
            "typescript" => lower.contains(".test.") || lower.contains(".spec."),
            "swift" => lower.ends_with("tests.swift"),
            _ => false,
        }
}

pub(crate) fn extensions(language: &str) -> &'static [&'static str] {
    match language {
        "rust" => &["rs"],
        "go" => &["go"],
        "python" => &["py"],
        "typescript" => &["ts", "tsx", "js", "jsx", "mts", "cts"],
        "cpp" => &["c", "cc", "cpp", "cxx", "h", "hh", "hpp"],
        "swift" => &["swift"],
        _ => &[],
    }
}

/// Whether a symbol's container is a trait implementation block (`impl Shape for Circle`):
/// its methods are reached through the trait, which reference search does not follow.
pub(crate) fn in_trait_impl(container: &str) -> bool {
    container
        .split(" > ")
        .any(|c| c.starts_with("impl ") && c.contains(" for "))
}

/// The candidates among a document's symbols, children included. An entry that is not a
/// symbol (no name or kind, a container name or children of the wrong shape, a candidate
/// without a readable position) is an error: skipped, it would leave a symbol unjudged in a
/// scan that claims to be complete.
/// The candidates among a document's symbols, children included, with full declaration spans.
pub fn collect_candidates(
    symbols: &[serde_json::Value],
    out: &mut Vec<CandidateSymbol>,
) -> std::result::Result<(), String> {
    for sym in symbols {
        let malformed = || crate::impact::unreadable("textDocument/documentSymbol", sym);
        let (Some(name), Some(kind)) = (
            sym.get("name").and_then(|n| n.as_str()),
            sym.get("kind").and_then(|k| k.as_u64()),
        ) else {
            return Err(malformed());
        };
        if name.is_empty() || !(1..=26).contains(&kind) {
            return Err(malformed());
        }
        let container = match sym.get("containerName") {
            None | Some(serde_json::Value::Null) => String::new(),
            Some(serde_json::Value::String(c)) => c.to_ascii_lowercase(),
            Some(_) => return Err(malformed()),
        };
        // Items inside a test module are tests, whatever they are called. rust-analyzer labels
        // modules by name ("tests"), other servers by kind and name.
        if container.split(" > ").any(|c| {
            let c = c.trim_start_matches("mod ");
            c == "tests" || c == "test" || c.ends_with("tests")
        }) {
            continue;
        }
        if let Some(kind_name) = kind_name(kind)
            && !name.is_empty()
        {
            let (line, col) = sym
                .get("selectionRange")
                .or_else(|| sym.get("range"))
                .or_else(|| sym.get("location").and_then(|l| l.get("range")))
                .and_then(|range| range.get("start"))
                .and_then(|start| {
                    crate::impact::one_based(start, "line")
                        .zip(crate::impact::one_based(start, "character"))
                })
                .ok_or_else(malformed)?;

            let range_val = sym
                .get("range")
                .or_else(|| sym.get("location").and_then(|l| l.get("range")));
            let range_start = range_val
                .and_then(|r| r.get("start"))
                .and_then(|s| {
                    crate::impact::one_based(s, "line")
                        .zip(crate::impact::one_based(s, "character"))
                })
                .unwrap_or((line, col));
            let range_end = range_val
                .and_then(|r| r.get("end"))
                .and_then(|e| {
                    crate::impact::one_based(e, "line")
                        .zip(crate::impact::one_based(e, "character"))
                })
                .unwrap_or((line, col));

            let kind_name = if kind_name == "method" && in_trait_impl(&container) {
                "trait-method"
            } else {
                kind_name
            };
            out.push(CandidateSymbol {
                name: name.to_string(),
                kind: kind_name.to_string(),
                line,
                col,
                range_start,
                range_end,
            });
        }
        match sym.get("children") {
            None | Some(serde_json::Value::Null) => {}
            Some(serde_json::Value::Array(children)) => collect_candidates(children, out)?,
            Some(_) => return Err(malformed()),
        }
    }
    Ok(())
}

pub(crate) fn collect(
    symbols: &[serde_json::Value],
    out: &mut Vec<(String, String, u32, u32)>,
) -> std::result::Result<(), String> {
    let mut detailed = Vec::new();
    collect_candidates(symbols, &mut detailed)?;
    out.extend(
        detailed
            .into_iter()
            .map(|c| (c.name, c.kind, c.line, c.col)),
    );
    Ok(())
}
