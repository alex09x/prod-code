/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::{SymbolHit, edit_distance};
use anyhow::Result;
use std::path::Path;

/// Whether a qualified request can name `hit`. A member belongs to the requested type when its
/// container names that type (including generic and impl labels); a free function belongs when
/// its file's module path has every qualifier. This deliberately never uses substring matching:
/// `Consumer::recv` must not resolve to `AsyncConsumer::recv`.
pub(crate) fn qualifier_matches(root: &Path, hit: &SymbolHit, qualifiers: &[&str]) -> bool {
    let decorated = decorated_owner(&hit.name);
    let mut owner = hit
        .container
        .as_deref()
        .map(owner_segments)
        .filter(|owner| !owner.is_empty())
        .unwrap_or_default();
    if !decorated.is_empty() {
        let already_ends_with = owner.len() >= decorated.len()
            && owner[owner.len() - decorated.len()..]
                .iter()
                .zip(&decorated)
                .all(|(a, b)| a.eq_ignore_ascii_case(b));
        if !already_ends_with {
            owner.extend(decorated);
        }
    }
    // rust-analyzer also labels methods as Function. A known owner constrains every kind;
    // the containing file cannot turn an explicitly owned member into a free function.
    if !owner.is_empty() {
        return owner_path_matches(root, &hit.path, qualifiers, &owner);
    }
    !symbol_is_type_member(hit.kind) && module_path_ends_with(root, &hit.path, qualifiers)
}

/// Kinds whose container is a type, never merely the module named by the hit's file.
pub(crate) fn symbol_is_type_member(kind: &str) -> bool {
    matches!(
        kind,
        "Method" | "Field" | "Property" | "EnumMember" | "Constructor" | "Event" | "Operator"
    )
}

/// The explicit owner decorating a server name (`pkg.Type.member`, `Type::member`).
pub(crate) fn decorated_owner(name: &str) -> Vec<String> {
    let bare = bare_symbol_name(name);
    name.rfind(bare)
        .map(|at| owner_segments(name[..at].trim_end_matches([':', '.', '/', '#'])))
        .unwrap_or_default()
}

/// Comparable owner segments from a container or outline label. Generic arguments and Rust
/// `impl` syntax describe the same owner and do not become path segments.
pub(crate) fn owner_segments(label: &str) -> Vec<String> {
    let label = label.trim();
    let target = match label.strip_prefix("impl") {
        Some(rest) if rest.starts_with([' ', '<']) => {
            let mut rest = rest.trim_start();
            if rest.starts_with('<') {
                let mut depth = 0usize;
                for (i, c) in rest.char_indices() {
                    match c {
                        '<' => depth += 1,
                        '>' => {
                            depth = depth.saturating_sub(1);
                            if depth == 0 {
                                rest = &rest[i + 1..];
                                break;
                            }
                        }
                        _ => {}
                    }
                }
            }
            let rest = rest.split(" where ").next().unwrap_or(rest);
            rest.rsplit_once(" for ").map_or(rest, |(_, ty)| ty)
        }
        _ => label,
    };
    let target = target
        .trim()
        .trim_start_matches('&')
        .trim()
        .strip_prefix("mut ")
        .unwrap_or(target.trim().trim_start_matches('&').trim())
        .trim();
    let target = [
        "pub struct ",
        "struct ",
        "class ",
        "enum ",
        "trait ",
        "interface ",
        "extension ",
    ]
    .into_iter()
    .find_map(|prefix| target.strip_prefix(prefix))
    .unwrap_or(target);
    let mut without_generics = String::with_capacity(target.len());
    let mut depth = 0usize;
    for c in target.chars() {
        match c {
            '<' => depth += 1,
            '>' if depth > 0 => depth -= 1,
            _ if depth == 0 => without_generics.push(c),
            _ => {}
        }
    }
    without_generics
        .replace(" > ", "::")
        .split(['.', ':', '/', '#'])
        .map(|part| {
            let part = part.trim();
            let part = part.strip_prefix("impl ").unwrap_or(part);
            let part = part.rsplit_once(" for ").map_or(part, |(_, ty)| ty);
            part.trim_matches(['(', ')', '*', '&']).replace('-', "_")
        })
        .filter(|part| !part.is_empty())
        .collect()
}

/// A requested owner may omit leading segments supplied by a server, but every segment the
/// caller did provide must agree. When the server only names the terminal type, its file must
/// establish the remaining module prefix.
pub(crate) fn owner_path_matches(
    root: &Path,
    path: &Path,
    requested: &[&str],
    declared: &[String],
) -> bool {
    let clean = |s: &str| s.trim_matches(['(', ')', '*', '&']).replace('-', "_");
    let equals = |a: &str, b: &str| clean(a).eq_ignore_ascii_case(&clean(b));
    let ends_with = |longer: &[String], shorter: &[&str]| {
        longer.len() >= shorter.len()
            && longer[longer.len() - shorter.len()..]
                .iter()
                .zip(shorter)
                .all(|(a, b)| equals(a, b))
    };
    if requested.is_empty() || declared.is_empty() {
        return false;
    }
    if ends_with(declared, requested) {
        return true;
    }
    if requested.len() < declared.len()
        || !declared
            .iter()
            .rev()
            .zip(requested.iter().rev())
            .all(|(a, b)| equals(a, b))
    {
        return false;
    }
    let module_prefix = &requested[..requested.len() - declared.len()];
    !module_prefix.is_empty() && module_path_ends_with(root, path, module_prefix)
}

/// Whether the module path `path`'s file gives it ends with `qualifiers`:
/// `crates/prod-code-mcp/src/report.rs` is `crates::prod_code_mcp::report`, which ends with
/// `prod_code_mcp::report` and with `report`. `src`, `lib`, `main`, `mod`, `__init__` and
/// `index` name no module of their own, and `-` is `_`.
pub(crate) fn module_path_ends_with(root: &Path, path: &Path, qualifiers: &[&str]) -> bool {
    if qualifiers.is_empty() {
        return false;
    }
    let rel = path.strip_prefix(root).unwrap_or(path).with_extension("");
    let segments: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().replace('-', "_"))
        .filter(|s| {
            !matches!(
                s.as_str(),
                "src" | "lib" | "main" | "mod" | "__init__" | "index"
            )
        })
        .collect();
    let ends_with = |qualifiers: &[&str]| {
        (qualifiers.is_empty() && segments.is_empty())
            || (!qualifiers.is_empty()
                && segments.len() >= qualifiers.len()
                && segments[segments.len() - qualifiers.len()..]
                    .iter()
                    .zip(qualifiers)
                    .all(|(segment, q)| segment.eq_ignore_ascii_case(&q.replace('-', "_"))))
    };
    ends_with(qualifiers)
        || (module_root_alias(root, path, qualifiers[0]) && ends_with(&qualifiers[1..]))
}

/// Whether `candidate` is a root segment the source itself establishes: Rust's `crate`, the
/// containing Cargo package/lib name, or the checkout directory name.
pub(crate) fn module_root_alias(root: &Path, path: &Path, candidate: &str) -> bool {
    let wanted = candidate.replace('-', "_");
    if root.file_name().is_some_and(|name| {
        name.to_string_lossy()
            .replace('-', "_")
            .eq_ignore_ascii_case(&wanted)
    }) {
        return true;
    }
    if cargo_manifest_names(root.join("Cargo.toml"), candidate) {
        return true;
    }
    // Test fixtures and macOS callers can spell a file through `/var` while their canonical
    // workspace root is below `/private/var`; compare canonical paths before walking upward.
    let canonical_path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut dir = canonical_path.parent();
    while let Some(current) = dir.filter(|current| current.starts_with(root)) {
        let manifest = current.join("Cargo.toml");
        if cargo_manifest_names(manifest, candidate) {
            return true;
        }
        if current == root {
            break;
        }
        dir = current.parent();
    }
    false
}

/// Whether a Cargo manifest establishes `candidate` as its crate keyword, library, or package.
pub(crate) fn cargo_manifest_names(manifest: std::path::PathBuf, candidate: &str) -> bool {
    let Ok(text) = std::fs::read_to_string(manifest) else {
        return false;
    };
    let Ok(manifest) = toml::from_str::<toml::Value>(&text) else {
        return false;
    };
    if candidate.eq_ignore_ascii_case("crate") {
        return manifest.get("package").is_some() || manifest.get("lib").is_some();
    }
    let wanted = candidate.replace('-', "_");
    ["package", "lib"].into_iter().any(|section| {
        manifest
            .get(section)
            .and_then(|table| table.get("name"))
            .and_then(toml::Value::as_str)
            .is_some_and(|name| name.replace('-', "_").eq_ignore_ascii_case(&wanted))
    })
}

/// How closely a symbol's name matches a query (#326): 0 the name itself, 1 a name that starts
/// with it, 2 one that holds it at a word boundary (`start_watcher` for `watcher`), 3 one that
/// holds it elsewhere, 4 one that only has its letters in order.
pub(crate) fn match_rank(name: &str, query: &str) -> u8 {
    let bare = bare_symbol_name(name);
    let lower = bare.to_ascii_lowercase();
    let query = bare_symbol_name(query).to_ascii_lowercase();
    if lower == query {
        return 0;
    }
    if lower.starts_with(&query) {
        return 1;
    }
    let Some(at) = lower.find(&query) else {
        return 4;
    };
    let before = bare[..at].chars().next_back();
    let first = bare[at..].chars().next();
    let boundary = matches!(before, Some('_' | ':' | '.'))
        || (before.is_some_and(|c| c.is_lowercase()) && first.is_some_and(|c| c.is_uppercase()));
    if boundary { 2 } else { 3 }
}

/// The one location `ties` point at, or an error listing them when they point at several.
pub(crate) fn single_candidate(
    root: &Path,
    symbol: &str,
    ties: &[&SymbolHit],
) -> Result<SymbolHit> {
    if ties.is_empty() {
        anyhow::bail!("no candidate found for `{symbol}`");
    }
    if ties.len() > 1
        && ties
            .iter()
            .any(|h| h.path != ties[0].path || h.line != ties[0].line)
    {
        let mut msg = format!(
            "`{symbol}` is ambiguous ({} candidates); qualify it (Type::name) or pass `path`:\n",
            ties.len()
        );
        for hit in ties.iter().take(10) {
            msg.push_str(&format!("  {}\n", hit.render(root)));
        }
        anyhow::bail!(msg.trim_end().to_string());
    }
    Ok(ties[0].clone())
}

/// A symbol name without the decoration some servers put around it: a trailing parameter list
/// (`bar()`, `bar(x: u32)`) and a leading qualifier (`Type.bar`, `Type::bar`, `(*T).Bar`).
/// What is left is the name a caller types.
pub(crate) fn bare_symbol_name(name: &str) -> &str {
    let mut name = name.trim();
    if name.ends_with(')') {
        // The `(` that opens the final parameter list, found by balancing from the end so
        // that a nested `fn(u8)` parameter does not cut the name short.
        let mut depth = 0usize;
        let mut open = None;
        for (i, c) in name.char_indices().rev() {
            match c {
                ')' => depth += 1,
                '(' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        open = Some(i);
                        break;
                    }
                }
                _ => {}
            }
        }
        if let Some(i) = open.filter(|&i| i > 0) {
            name = name[..i].trim_end();
        }
    }
    let after_path = name.rfind("::").map(|i| i + 2);
    let after_dot = name.rfind('.').map(|i| i + 1);
    match after_path.max(after_dot) {
        Some(i) if i < name.len() => &name[i..],
        _ => name,
    }
}

/// The error for a name nothing in the index is called. The first sentence stays the same
/// whatever follows it; the closest names the index returned are listed so that the caller can
/// pick the symbol it meant instead of the tool guessing one.
pub(crate) fn no_symbol_message(symbol: &str, name: &str, others: &[SymbolHit]) -> String {
    let wanted = name.to_ascii_lowercase();
    let mut close: Vec<(usize, &str)> = Vec::new();
    for hit in others {
        let bare = bare_symbol_name(&hit.name);
        if !close.iter().any(|(_, n)| *n == bare) {
            close.push((edit_distance(&wanted, &bare.to_ascii_lowercase()), bare));
        }
    }
    close.sort();
    let close: Vec<&str> = close.into_iter().take(5).map(|(_, n)| n).collect();
    if close.is_empty() {
        format!(
            "no symbol named `{symbol}` in the workspace index (try code_symbols with a shorter name)"
        )
    } else {
        format!(
            "no symbol named `{symbol}` in the workspace index; did you mean: {}",
            close.join(", ")
        )
    }
}
