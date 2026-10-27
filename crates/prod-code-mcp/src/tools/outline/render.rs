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

/// The start and end (line, character) of a symbol, 0-based. Lines alone cannot tell
/// `fn f() { let x = 1; }`'s local from the function that holds it (#1403).
fn symbol_span(sym: &serde_json::Value) -> ((u64, u64), (u64, u64)) {
    let range = sym
        .get("range")
        .or_else(|| sym.get("location").and_then(|l| l.get("range")));
    let at = |edge: &str| {
        let point = range.and_then(|r| r.get(edge))?;
        let line = point.get("line").and_then(|l| l.as_u64())?;
        Some((
            line,
            point.get("character").and_then(|c| c.as_u64()).unwrap_or(0),
        ))
    };
    let start = at("start").unwrap_or((0, 0));
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
        let bodies: Vec<_> = entries
            .iter()
            .map(|(_, sym, _)| *sym)
            .filter(|s| matches!(s.get("kind").and_then(|k| k.as_u64()), Some(6 | 12)))
            .map(symbol_span)
            .collect();
        let mut skipped_locals = 0usize;
        // Whether each type the file declares at the top level is exported: an `impl` of a
        // private type exports nothing, whatever its members say (#1587).
        let types: std::collections::HashMap<&str, bool> = entries
            .iter()
            .filter(|(depth, sym, _)| {
                *depth == 1
                    && matches!(
                        sym.get("kind").and_then(|k| k.as_u64()),
                        Some(5 | 10 | 11 | 23)
                    )
            })
            .filter_map(|(_, sym, _)| {
                let name = sym.get("name").and_then(|n| n.as_str())?;
                let (line, _) = symbol_lines(sym);
                let declaration = source_lines.get(line as usize).copied().unwrap_or("");
                Some((name, is_exported(language, name, declaration, 1)))
            })
            .collect();
        // Whether each enclosing symbol is exported, and its kind, by depth (#1418).
        let mut owners: Vec<(bool, u64)> = Vec::new();
        for (depth, sym, in_body) in entries {
            let name = sym.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let kind = sym.get("kind").and_then(|k| k.as_u64()).unwrap_or(0);
            let name = crate::tools::symbols::bindings::binding_name(Path::new(path), kind, name);
            let (line, _) = symbol_lines(sym);
            let declaration = source_lines.get(line as usize).copied().unwrap_or("");
            owners.truncate(depth.saturating_sub(1));
            let exported =
                exported_with_owners(language, name, declaration, depth, &owners, &types);
            owners.push((exported, kind));
            // Locals (LSP kind 13, Variable, inside a body) are noise for a structural
            // outline: a 2000-line file lists hundreds of them.
            // A flat answer has no nesting: a variable is a local when it starts inside a
            // function's range, on the function's own line too.
            let (at, _) = symbol_span(sym);
            let local = kind == 13 && (in_body || bodies.iter().any(|(s, e)| *s < at && at <= *e));
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
            if options.exported_only && !exported {
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
        } else if listed == 0 {
            out.push_str("  (no outline symbols available)\n");
        }
    } else {
        out.push_str("No outline symbols available.");
    }
    (out.trim_end().to_string(), listed)
}

/// Whether a symbol is exported once its owners are counted (#1418): a member of an owner that
/// is not exported is not, and in Rust a trait's items and an enum's variants are as visible as
/// their owner without a `pub` of their own. `owners` holds each enclosing symbol's verdict and
/// kind. A Rust `impl` and a Swift `extension` have no visibility of their own and pass their
/// owners' on, unless the type it implements, or the trait, is declared in the file and not
/// exported (`types`, #1587).
fn exported_with_owners(
    language: Option<&str>,
    name: &str,
    declaration: &str,
    depth: usize,
    owners: &[(bool, u64)],
    types: &std::collections::HashMap<&str, bool>,
) -> bool {
    let owners_exported = owners.iter().all(|(exported, _)| *exported);
    let head = declaration.trim_start();
    let transparent = match language {
        Some("rust") => name.starts_with("impl ") || head.starts_with("impl"),
        Some("swift") => head.split_whitespace().any(|w| w == "extension"),
        _ => false,
    };
    if transparent {
        let header = if name.starts_with("impl") { name } else { head };
        let private = |ty: &str| types.get(ty).is_some_and(|exported| !exported);
        let hidden = language == Some("rust") && {
            let (implemented, self_type) = impl_types(header);
            private(self_type) || implemented.is_some_and(private)
        };
        return owners_exported && !hidden;
    }
    let inherited = language == Some("rust") && matches!(owners.last(), Some((true, 10 | 11)));
    owners_exported && (inherited || is_exported(language, name, declaration, depth))
}

/// The trait and the self type a Rust `impl` header names, each by its last path segment
/// without generic arguments: `impl<'a> fmt::Display for Wrapper<'a> {` is
/// `(Some("Display"), "Wrapper")`, `impl Hidden` is `(None, "Hidden")`.
fn impl_types(header: &str) -> (Option<&str>, &str) {
    let mut rest = header.trim_start().strip_prefix("impl").unwrap_or(header);
    // The impl's own generic parameters, `<T: Clone, U>`, nested brackets included.
    if let Some(generics) = rest.trim_start().strip_prefix('<') {
        let mut depth = 1;
        let end = generics.char_indices().find_map(|(i, c)| {
            match c {
                '<' => depth += 1,
                '>' => depth -= 1,
                _ => {}
            }
            (depth == 0).then_some(i + 1)
        });
        rest = &generics[end.unwrap_or(generics.len())..];
    }
    let rest = rest.split(['{', '\n']).next().unwrap_or("");
    let rest = rest.split(" where ").next().unwrap_or(rest);
    fn type_name(ty: &str) -> &str {
        let ty = ty.trim().trim_start_matches(['&', '!']).trim_start();
        let ty = ty.strip_prefix("mut ").unwrap_or(ty);
        let ty = ty.strip_prefix("dyn ").unwrap_or(ty);
        let path = ty.split('<').next().unwrap_or(ty);
        path.rsplit("::").next().unwrap_or(path).trim()
    }
    match rest.split_once(" for ") {
        Some((implemented, self_type)) => (Some(type_name(implemented)), type_name(self_type)),
        None => (None, type_name(rest)),
    }
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
