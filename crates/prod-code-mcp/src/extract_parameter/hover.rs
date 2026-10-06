/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::path::Path;

use super::syntax::Syntax;

/// The type in a hover answer, when it is one this can read.
///
/// rust-analyzer writes `let x: u32` for a binding and a bare path for a type, and neither
/// shape is reliable for an arbitrary expression — so a hover that does not parse is a reason
/// to ask the caller for the type rather than to guess at it.
pub fn type_from_hover(hover: &str) -> Option<String> {
    for line in hover.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("let ")
            && let Some((_, ty)) = rest.split_once(':')
        {
            let ty = ty.trim().trim_end_matches(&[',', ';'][..]).trim();
            if !ty.is_empty() {
                return Some(ty.to_string());
            }
        }
    }
    None
}

/// The type in a clangd hover for a binding: the `Type:` line under a `### variable`,
/// `### param` or `### field` heading. clangd follows a typedef with what it stands for,
/// `std::size_t (aka unsigned long)`, and the name the code wrote is the one kept. A `const`
/// on the whole type is dropped: on a parameter passed by value it binds nothing the caller
/// can see. A type with a parenthesis or a bracket in it (a lambda, an array, a function
/// pointer) cannot be written before a name, so it gives none.
pub fn clangd_type(hover: &str) -> Option<String> {
    const BINDINGS: [&str; 5] = [
        "### variable ",
        "### param ",
        "### field ",
        "### static-property ",
        "### instance-property ",
    ];
    let mut lines = hover.lines().map(str::trim);
    let heading = lines.next()?;
    if !BINDINGS.iter().any(|b| heading.starts_with(b)) {
        return None;
    }
    let ty = lines.find_map(|l| l.strip_prefix("Type: `")?.strip_suffix('`'))?;
    let ty = ty.split(" (aka ").next().unwrap_or(ty).trim();
    let ty = match ty.strip_prefix("const ") {
        Some(inner) if !inner.contains(['*', '&']) => inner,
        _ => ty,
    };
    if ty.is_empty() || ty.contains(['(', '[']) {
        return None;
    }
    Some(ty.to_string())
}

/// The type in a sourcekit-lsp hover for a binding: `let width: Int`, `var entries: [Int]`,
/// perhaps after modifiers (`public`, `static`, `private(set)`, `@MainActor`) and before an
/// accessor block (`{ get }`) or an initial value.
pub fn swift_type(hover: &str) -> Option<String> {
    for line in hover.lines() {
        let line = line.trim();
        let rest = ["let ", "var "].iter().find_map(|keyword| {
            let at = if line.starts_with(keyword) {
                0
            } else {
                line.find(&format!(" {keyword}"))? + 1
            };
            line[..at]
                .split_whitespace()
                .all(|w| {
                    w.starts_with('@')
                        || w.chars()
                            .all(|c| c.is_ascii_lowercase() || c == '(' || c == ')')
                })
                .then(|| &line[at + keyword.len()..])
        });
        let Some((_, ty)) = rest.and_then(|r| r.split_once(':')) else {
            continue;
        };
        let ty = ty.split(" {").next().unwrap_or(ty);
        let ty = ty.split(" = ").next().unwrap_or(ty).trim();
        if !ty.is_empty() {
            return Some(ty.to_string());
        }
    }
    None
}

/// The type in a TypeScript hover for a binding: `const width: 80`, `let n: number`,
/// `(parameter) text: string`, `(property) Store.entries: number[]`. A literal type is widened,
/// because a parameter typed `80` would accept nothing else. A function or a method is not a
/// binding: its hover names what it returns, not what it is.
pub fn typescript_type(hover: &str) -> Option<String> {
    const BINDINGS: [&str; 6] = [
        "const ",
        "let ",
        "var ",
        "(parameter) ",
        "(property) ",
        "(variable) ",
    ];
    for line in hover.lines() {
        let line = line.trim();
        let Some(rest) = BINDINGS.iter().find_map(|p| line.strip_prefix(p)) else {
            continue;
        };
        let Some((name, ty)) = rest.split_once(':') else {
            continue;
        };
        let ty = ty.trim().trim_end_matches(';').trim();
        if name.contains('(') || ty.is_empty() {
            continue;
        }
        // An object type is written over several lines, and its first line is not a type.
        let opened = ty.matches(['{', '(', '[', '<']).count();
        let closed = ty.matches(['}', ')', ']']).count() + ty.matches('>').count()
            - ty.matches("=>").count();
        if opened != closed {
            return None;
        }
        return Some(
            Syntax::TypeScript
                .literal_type(ty)
                .map(str::to_string)
                .unwrap_or_else(|| ty.to_string()),
        );
    }
    None
}

/// The type in a basedpyright hover for a binding: `(variable) width: Literal[80]`,
/// `(constant) WIDTH: Literal[80]`, `(parameter) text: str`. `Literal[80]` is widened to `int`
/// for the reason TypeScript's `80` is; a type the checker made up, like `Self@Store` or one with
/// `Unknown` in it, cannot be written in an annotation and gives none.
pub fn python_type(hover: &str) -> Option<String> {
    const BINDINGS: [&str; 3] = ["(variable) ", "(constant) ", "(parameter) "];
    for line in hover.lines() {
        let line = line.trim();
        let Some(rest) = BINDINGS.iter().find_map(|p| line.strip_prefix(p)) else {
            continue;
        };
        let Some((_, ty)) = rest.split_once(':') else {
            continue;
        };
        let ty = ty.trim();
        if ty.is_empty() || ty.contains('@') || ty.contains("Unknown") {
            return None;
        }
        if let Some(values) = ty
            .strip_prefix("Literal[")
            .and_then(|v| v.strip_suffix(']'))
        {
            let kinds: BTreeSet<&str> = values
                .split(',')
                .map(|v| Syntax::Python.literal_type(v).unwrap_or("?"))
                .collect();
            return match kinds.into_iter().collect::<Vec<_>>().as_slice() {
                [kind] if *kind != "?" => Some(kind.to_string()),
                _ => Some(ty.to_string()),
            };
        }
        return Some(ty.to_string());
    }
    None
}

/// The type in a gopls hover for a binding: `var width int`, `field entries []int`,
/// `const Base untyped int = 80`. An untyped constant takes its default type, which is what a
/// variable initialised from it would have.
pub fn go_type(hover: &str) -> Option<String> {
    const BINDINGS: [&str; 3] = ["var ", "field ", "const "];
    for line in hover.lines() {
        let line = line.trim();
        let Some(rest) = BINDINGS.iter().find_map(|p| line.strip_prefix(p)) else {
            continue;
        };
        let Some((_, ty)) = rest.split_once(' ') else {
            continue;
        };
        let ty = ty.split(" = ").next().unwrap_or("").trim();
        let ty = match ty.strip_prefix("untyped ") {
            Some("float") => "float64",
            Some("complex") => "complex128",
            Some(kind) => kind,
            None => ty,
        };
        if !ty.is_empty() {
            return Some(ty.to_string());
        }
    }
    None
}

/// The type a hover gives for the selection `from..to`, outside Rust.
///
/// A hover describes one token. When the selection is longer than the token the hover
/// covers, the type is the token's, not the expression's (in `name + 1`, `name` may be a string
/// and the sum a number), so it is taken only when the two spans are the same.
pub async fn hover_type(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    text: &str,
    from: usize,
    to: usize,
    syntax: Syntax,
) -> Option<String> {
    let selected = &text[from..to];
    let start = from + (selected.len() - selected.trim_start().len());
    let end = from + selected.trim_end().len();
    let (line, col) = crate::signature::line_col_at(text, start)?;
    let hover = crate::tools::execute_lsp_query(
        remote,
        root,
        file,
        "textDocument/hover",
        serde_json::json!({
            "textDocument": { "uri": url::Url::from_file_path(file).ok()?.to_string() },
            "position": { "line": line - 1, "character": col - 1 },
        }),
    )
    .await
    .ok()?;
    if let Some(range) = hover.get("range") {
        let at = |pointer: &str| {
            range
                .pointer(pointer)
                .and_then(|v| v.as_u64())
                .map(|v| v as u32 + 1)
        };
        let covered = (
            at("/start/line").zip(at("/start/character")),
            at("/end/line").zip(at("/end/character")),
        );
        if covered
            != (
                Some((line, col)),
                Some(crate::signature::line_col_at(text, end)?),
            )
        {
            return None;
        }
    } else if !text[start..end]
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_')
    {
        // sourcekit-lsp answers without a range, so nothing says how much of the selection
        // the hover describes. Only a selection that is a single name is certainly the token
        // it was asked about.
        return None;
    }
    syntax.type_from_hover(hover.pointer("/contents/value")?.as_str()?)
}
