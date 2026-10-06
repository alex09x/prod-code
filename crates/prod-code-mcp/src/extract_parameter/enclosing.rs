/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::Result;

use super::types::Enclosing;

/// The smallest *function* containing `line`, and its line span.
///
/// Not the smallest declaration: `textDocument/documentSymbol` reports local bindings too, so
/// the innermost thing containing an expression is usually the `let` it is part of. Only a
/// function or a method can take a parameter, so only those are candidates.
pub fn enclosing_function(symbols: &serde_json::Value, line: u32) -> Option<(String, u32, u32)> {
    enclosing_declaration(symbols, line).map(|d| (d.name, d.start, d.end))
}

/// [`enclosing_function`], with the name's position and the exact end of the range.
pub fn enclosing_declaration(symbols: &serde_json::Value, line: u32) -> Option<Enclosing> {
    /// LSP `SymbolKind`: a free function, and a method on a type.
    const FUNCTION: u64 = 12;
    const METHOD: u64 = 6;

    fn walk(nodes: &[serde_json::Value], line: u32, best: &mut Option<Enclosing>) {
        for node in nodes {
            let range = node
                .get("range")
                .or_else(|| node.get("location").and_then(|l| l.get("range")));
            let kind = node.get("kind").and_then(|k| k.as_u64()).unwrap_or(0);
            if let Some(range) = range
                && (kind == FUNCTION || kind == METHOD)
                && let (Some(s), Some(e)) = (
                    range.pointer("/start/line").and_then(|l| l.as_u64()),
                    range.pointer("/end/line").and_then(|l| l.as_u64()),
                )
            {
                let (s, e) = (s as u32 + 1, e as u32 + 1);
                if s <= line && line <= e && best.as_ref().is_none_or(|b| e - s < b.end - b.start) {
                    let at = |pointer: &str| {
                        node.pointer(pointer)
                            .and_then(|v| v.as_u64())
                            .map(|v| v as u32 + 1)
                    };
                    *best = Some(Enclosing {
                        name: node
                            .get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        start: s,
                        end: e,
                        end_col: range
                            .pointer("/end/character")
                            .and_then(|c| c.as_u64())
                            .map_or(1, |c| c as u32 + 1),
                        name_at: at("/selectionRange/start/line")
                            .zip(at("/selectionRange/start/character")),
                    });
                }
            }
            if let Some(children) = node.get("children").and_then(|c| c.as_array()) {
                walk(children, line, best);
            }
        }
    }
    let mut best = None;
    walk(symbols.as_array().map(|a| a.as_slice())?, line, &mut best);
    best
}

/// The offset of a function's name in a language whose declarations do not start with `fn`.
///
/// The analyzer's `selectionRange` is the name itself, and it is trusted when the text there
/// says so; gopls calls a method `(*Store).Limit` but selects only `Limit`, which is why `bare`
/// is the name after the last dot. Without it, the first whole-word occurrence of the name
/// from the declaration's first line on that is followed by a parameter list.
pub(crate) fn name_offset(
    text: &str,
    bare: &str,
    start: u32,
    name_at: Option<(u32, u32)>,
) -> Option<usize> {
    if let Some((line, col)) = name_at
        && let Some(at) = crate::signature::offset_of(text, line, col)
        && text[at..].starts_with(bare)
    {
        return Some(at);
    }
    let from = crate::signature::offset_of(text, start, 1)?;
    let is_word = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    text[from..]
        .match_indices(bare)
        .map(|(i, _)| from + i)
        .find(|&at| {
            let before = text[..at].chars().next_back();
            let after = text[at + bare.len()..].trim_start().chars().next();
            !before.is_some_and(is_word) && matches!(after, Some('(' | '<' | '['))
        })
}

/// The span between the parentheses of the parameter list that follows a function's name, in
/// a language other than Rust. Type parameters come first and are skipped: `<T>` in TypeScript,
/// `[T any]` in Go and `[T]` in Python.
pub(crate) fn parameter_list(text: &str, name_end: usize) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut i = name_end;
    loop {
        while bytes.get(i).is_some_and(|b| b.is_ascii_whitespace()) {
            i += 1;
        }
        match bytes.get(i)? {
            b'(' => {
                let close = crate::parameter_object::matching_bracket(text, i)?;
                return Some((i + 1, close));
            }
            b'[' => i = crate::parameter_object::matching_bracket(text, i)? + 1,
            b'<' => {
                // `=>` in a bound such as `<F extends () => void>` does not close anything.
                let mut depth = 0i32;
                loop {
                    match bytes.get(i)? {
                        b'<' => depth += 1,
                        b'>' if i == 0 || bytes[i - 1] != b'=' => depth -= 1,
                        _ => {}
                    }
                    i += 1;
                    if depth == 0 {
                        break;
                    }
                }
            }
            _ => return None,
        }
    }
}

/// The parameter list with `param` added at the end.
pub fn with_parameter(list: &str, param: &str) -> String {
    let trimmed = list.trim();
    if trimmed.is_empty() {
        return param.to_string();
    }
    // A trailing comma means the list is written one per line; keep that shape, and keep
    // whatever whitespace sits between the last parameter and the closing parenthesis.
    if trimmed.ends_with(',') {
        let head = list.trim_end_matches(|c: char| c.is_whitespace());
        let tail = &list[head.len()..];
        let indent: String = head
            .lines()
            .next_back()
            .unwrap_or("")
            .chars()
            .take_while(|c| c.is_whitespace())
            .collect();
        return format!("{head}\n{indent}{param},{tail}");
    }
    format!("{trimmed}, {param}")
}

/// The argument list with `argument` added at the end.
pub fn with_argument(args: &str, argument: &str) -> String {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return argument.to_string();
    }
    format!("{trimmed}, {argument}")
}

/// Where the function named at `file:line:col` is declared, as (file, line, column), 1-based.
/// clangd answers with the definition itself when there is no separate declaration, and with a
/// `LocationLink` or a single `Location` as readily as with a list. A failed request or an entry
/// without a position is an error: a declaration left without the parameter no longer matches
/// its definition (#446).
pub(crate) async fn declarations(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
) -> Result<Vec<(PathBuf, u32, u32)>> {
    let uri = url::Url::from_file_path(file)
        .map_err(|_| anyhow::anyhow!("invalid path {}", file.display()))?;
    let answer = crate::tools::execute_lsp_query(
        remote,
        root,
        file,
        "textDocument/declaration",
        serde_json::json!({
            "textDocument": { "uri": uri.to_string() },
            "position": { "line": line - 1, "character": col - 1 },
        }),
    )
    .await?;
    crate::refactor::lsp_locations(&answer, "declarations")
}

/// The report's note on files that call the function by name but were not reported by the
/// analyzer (#294), or nothing when there are none. sourcekit-lsp finds references in the index
/// a build writes, so for Swift the note says to build first.
pub fn unreported_note(unreported: &[String], file: &str) -> String {
    if unreported.is_empty() {
        return String::new();
    }
    let hint = if file.ends_with(".swift") {
        "; sourcekit-lsp finds references in the index a build writes: run code_check \
         (swift build) and ask again"
    } else {
        ""
    };
    let mut out = format!(
        "\nchecked, not rewritten ({} file(s) that call it by name where the analyzer reported \
         no reference{hint}):\n",
        unreported.len()
    );
    for f in unreported {
        out.push_str(&format!("  {f}\n"));
    }
    out
}
