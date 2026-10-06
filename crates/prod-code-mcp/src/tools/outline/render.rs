/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;

use super::options::OutlineOptions;

/// The start and end line (0-based) of a symbol from `textDocument/documentSymbol`.
pub(crate) fn symbol_lines(sym: &serde_json::Value) -> (u64, u64) {
    let range = sym
        .get("range")
        .or_else(|| sym.get("location").and_then(|l| l.get("range")));
    let at = |edge: &str| {
        range
            .and_then(|r| r.get(edge))
            .and_then(|s| s.get("line"))
            .and_then(|l| l.as_u64())
    };
    let start = at("start").unwrap_or(0);
    (start, at("end").unwrap_or(start).max(start))
}

/// A file's outline from its `textDocument/documentSymbol` answer, for the MCP tool and the
/// CLI alike. A variable inside a function or method is a local and is left out unless
/// `include_locals`; a top-level `static`, which the analyzer reports with the same kind, is not
/// inside one and stays. `hint` says how to list the locals anyway.
pub fn render_outline(
    res: &serde_json::Value,
    path: &str,
    max_depth: usize,
    include_locals: bool,
    hint: &str,
) -> String {
    render_outline_with(
        res,
        path,
        &OutlineOptions::all(max_depth, include_locals, hint),
        None,
    )
    .0
}

/// [`render_outline`] with the kind and export filters of `options`, reading `source` (the
/// file's text) to tell what is exported. Returns the text and how many symbols it lists.
pub fn render_outline_with(
    res: &serde_json::Value,
    path: &str,
    options: &OutlineOptions,
    source: Option<&str>,
) -> (String, usize) {
    let source_lines: Vec<&str> = source.map(|s| s.lines().collect()).unwrap_or_default();
    let language = crate::sync::engine_for_file(Path::new(path));
    let mut listed = 0usize;
    let mut out = String::new();
    if let Some(arr) = res.as_array() {
        out.push_str(&format!("Outline for {path}:\n"));
        let mut entries = Vec::new();
        outline_entries(arr, 1, false, &mut entries);
        let bodies: Vec<(u64, u64)> = entries
            .iter()
            .map(|(_, sym, _)| *sym)
            .filter(|s| matches!(s.get("kind").and_then(|k| k.as_u64()), Some(6 | 12)))
            .map(symbol_lines)
            .collect();
        let mut skipped_locals = 0usize;
        for (depth, sym, in_body) in entries {
            let name = sym.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let kind = sym.get("kind").and_then(|k| k.as_u64()).unwrap_or(0);
            // Locals (LSP kind 13, Variable, inside a body) are noise for a structural
            // outline: a 2000-line file lists hundreds of them.
            let (line, _) = symbol_lines(sym);
            let local =
                kind == 13 && (in_body || bodies.iter().any(|(s, e)| *s < line && line <= *e));
            if local && !options.include_locals {
                skipped_locals += 1;
                continue;
            }
            if depth > options.max_depth {
                continue;
            }
            let kind_str = match kind {
                2 => "Module",
                5 => "Class",
                6 => "Method",
                7 => "Property",
                8 => "Field",
                9 => "Constructor",
                10 => "Enum",
                11 => "Interface",
                12 => "Function",
                13 => "Variable",
                14 => "Constant",
                22 => "EnumMember",
                23 => "Struct",
                _ => "Symbol",
            };
            if let Some(kinds) = &options.kinds
                && !kinds.iter().any(|k| k.eq_ignore_ascii_case(kind_str))
            {
                continue;
            }
            let declaration = source_lines.get(line as usize).copied().unwrap_or("");
            if options.exported_only && !is_exported(language, name, declaration, depth) {
                continue;
            }
            listed += 1;
            out.push_str(&format!("  [{kind_str}] {name} (line {})\n", line + 1));
        }
        if skipped_locals > 0 {
            out.push_str(&format!(
                "  ({skipped_locals} local variable(s) hidden; {} to list them)\n",
                options.hint
            ));
        }
    } else {
        out.push_str("No outline symbols available.");
    }
    (out.trim_end().to_string(), listed)
}

/// Whether a symbol is part of what its file exports, by its language's rule (#368): Go's
/// capital letter (a method's receiver type too), Rust's `pub`, Swift's `public` and `open`,
/// TypeScript's `export` at the top level and no `private`/`protected` below it, Python's names
/// without a leading underscore (dunder methods count). `declaration` is the text of the
/// symbol's line; `depth` its nesting, 1 for the top level.
pub(crate) fn is_exported(
    language: Option<&str>,
    name: &str,
    declaration: &str,
    depth: usize,
) -> bool {
    let decl = declaration.trim_start();
    let capital = |s: &str| {
        s.trim_start_matches(['*', '&', '(', '.'])
            .chars()
            .next()
            .is_some_and(char::is_uppercase)
    };
    match language {
        Some("go") => match name.strip_prefix('(').and_then(|r| r.split_once(')')) {
            Some((receiver, method)) => capital(receiver) && capital(method),
            None => capital(name),
        },
        Some("rust") => decl.starts_with("pub ") || decl.starts_with("pub("),
        Some("python") => {
            !name.starts_with('_') || (name.starts_with("__") && name.ends_with("__"))
        }
        Some("swift") => decl
            .split_whitespace()
            .any(|w| w == "public" || w == "open"),
        Some("typescript") => {
            if depth <= 1 {
                decl.starts_with("export ")
            } else {
                !name.starts_with('#')
                    && !decl
                        .split_whitespace()
                        .any(|w| w == "private" || w == "protected")
            }
        }
        _ => !decl.starts_with("static "),
    }
}

/// The symbols of a `textDocument/documentSymbol` answer in document order, each with its depth
/// and whether it sits in a function's body. A flat answer (the gateway's, for Rust) gives the
/// depth as a container chain ("a > b"); a nested one (`children`, as sourcekit-lsp, clangd,
/// pyright and the TypeScript server answer) by its nesting, whose members an outline used to
/// leave out (#358).
pub(crate) fn outline_entries<'a>(
    symbols: &'a [serde_json::Value],
    depth: usize,
    in_body: bool,
    out: &mut Vec<(usize, &'a serde_json::Value, bool)>,
) {
    for symbol in symbols {
        let chained = symbol
            .get("containerName")
            .and_then(|c| c.as_str())
            .filter(|c| c.contains(" > "))
            .map(|c| c.split(" > ").count() + 1);
        out.push((chained.unwrap_or(depth), symbol, in_body));
        if let Some(children) = symbol.get("children").and_then(|c| c.as_array()) {
            let body = in_body
                || matches!(
                    symbol.get("kind").and_then(|k| k.as_u64()),
                    Some(6 | 9 | 12)
                );
            outline_entries(children, depth + 1, body, out);
        }
    }
}
