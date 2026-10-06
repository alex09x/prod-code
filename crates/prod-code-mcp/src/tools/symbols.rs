/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Workspace symbol resolution, search across projects, and LSP coordinate translation.

use std::net::SocketAddr;
use std::path::Path;

use anyhow::Result;
use url::Url;

use super::execute_lsp_query;

/// One `workspace/symbol` hit, positioned on the symbol's name (1-based).
#[derive(Debug, Clone)]
pub struct SymbolHit {
    pub path: std::path::PathBuf,
    pub name: String,
    pub kind: &'static str,
    pub container: Option<String>,
    pub line: u32,
    pub col: u32,
}

impl SymbolHit {
    pub fn render(&self, root: &Path) -> String {
        let rel = self.path.strip_prefix(root).unwrap_or(&self.path).display();
        let container = self
            .container
            .as_deref()
            .map(|c| format!("{c}::"))
            .unwrap_or_default();
        format!(
            "[{}] {container}{} — {rel}:{}:{}",
            self.kind, self.name, self.line, self.col
        )
    }
}

pub(crate) fn symbol_kind_name(kind: u64) -> &'static str {
    match kind {
        1 => "File",
        2 => "Module",
        3 => "Namespace",
        4 => "Package",
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
        15 => "String",
        16 => "Number",
        17 => "Boolean",
        18 => "Array",
        19 => "Object",
        20 => "Key",
        21 => "Null",
        22 => "EnumMember",
        23 => "Struct",
        24 => "Event",
        25 => "Operator",
        26 => "TypeParameter",
        _ => "Symbol",
    }
}

/// An LSP position is zero-based, but every tool position we retain is one-based.  Keep a
/// distinct error type so a nested-project search can still ignore an unavailable server while
/// refusing evidence that a server did return but encoded incorrectly.
#[derive(Debug)]
pub(crate) struct MalformedLspCoordinate(String);

impl std::fmt::Display for MalformedLspCoordinate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for MalformedLspCoordinate {}

pub(crate) fn lsp_coordinate(start: &serde_json::Value, field: &str, context: &str) -> Result<u32> {
    let value = start.get(field).ok_or_else(|| {
        anyhow::Error::new(MalformedLspCoordinate(format!(
            "malformed LSP {context}: missing `{field}` coordinate"
        )))
    })?;
    let value = value.as_u64().ok_or_else(|| {
        anyhow::Error::new(MalformedLspCoordinate(format!(
            "malformed LSP {context}: `{field}` must be a non-negative integer"
        )))
    })?;
    let value = u32::try_from(value).map_err(|_| {
        anyhow::Error::new(MalformedLspCoordinate(format!(
            "malformed LSP {context}: `{field}` exceeds u32"
        )))
    })?;
    value.checked_add(1).ok_or_else(|| {
        anyhow::Error::new(MalformedLspCoordinate(format!(
            "malformed LSP {context}: `{field}` cannot be converted to a one-based coordinate"
        )))
    })
}

pub(crate) fn lsp_position(start: &serde_json::Value, context: &str) -> Result<(u32, u32)> {
    Ok((
        lsp_coordinate(start, "line", context)?,
        lsp_coordinate(start, "character", context)?,
    ))
}

pub(crate) fn is_malformed_lsp_coordinate(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.is::<MalformedLspCoordinate>())
}

/// `workspace/symbol` through the pooled session of the project `hint` belongs to (the root
/// when absent). Hits without a range (LSP `WorkspaceSymbol` without resolve) are skipped.
pub async fn workspace_symbol_search(
    remote: SocketAddr,
    root: &Path,
    query: &str,
    hint: Option<&Path>,
    limit: usize,
) -> Result<Vec<SymbolHit>> {
    workspace_symbol_search_with_retry_policy(remote, root, query, hint, limit, true).await
}

pub(crate) async fn workspace_symbol_search_auxiliary(
    remote: SocketAddr,
    root: &Path,
    query: &str,
    hint: Option<&Path>,
    limit: usize,
) -> Result<Vec<SymbolHit>> {
    workspace_symbol_search_with_retry_policy(remote, root, query, hint, limit, false).await
}

pub(crate) async fn workspace_symbol_search_with_retry_policy(
    remote: SocketAddr,
    root: &Path,
    query: &str,
    hint: Option<&Path>,
    limit: usize,
    retry_empty_answer: bool,
) -> Result<Vec<SymbolHit>> {
    // The LSP servers (tsc, clangd, pyright) index a project once one of its files is open;
    // the session opens the anchor file before the query, so pick a real source file when the
    // caller gave none or a directory.
    let anchor = match hint {
        Some(h) if h.is_file() => h.to_path_buf(),
        Some(h) => representative_source_file(h).unwrap_or_else(|| h.to_path_buf()),
        None => representative_source_file(root).unwrap_or_else(|| root.to_path_buf()),
    };
    let params = serde_json::json!({ "query": query, "limit": limit.max(1) });
    let asked = std::time::Instant::now();
    let mut res =
        execute_lsp_query(remote, root, &anchor, "workspace/symbol", params.clone()).await?;
    // How patient to be with an empty answer depends on how long before the question the engine
    // was loaded (#381). A warm engine's empty answer is the answer: a miss used to sleep 800 ms
    // in every project it asked. One loaded moments before may still be indexing and is asked
    // again a few times, longer each time; a gateway that does not say gets the one retry it
    // always had. The age is taken at the question: a fresh gopls on a busy node took a minute
    // to give its first answer, and was no warmer for it.
    let age = crate::session::pooled_engine_age(remote, root, &anchor)
        .await
        .map(|age| age.saturating_sub(asked.elapsed()));
    // A gateway that holds index questions until the server is ready has already waited: its
    // empty answer is final (#391). The age is the guess for the others.
    let gated = crate::session::pooled_index_gated(remote, root, &anchor).await;
    let retries = match age {
        _ if gated => 0,
        _ if !retry_empty_answer => 0,
        Some(age) if age >= crate::session::INDEXING_GRACE => 0,
        Some(_) => 3,
        None => 1,
    };
    let mut pause = std::time::Duration::from_millis(800);
    for _ in 0..retries {
        if res.as_array().is_some_and(|a| !a.is_empty()) {
            break;
        }
        tokio::time::sleep(pause).await;
        pause *= 2;
        res = execute_lsp_query(remote, root, &anchor, "workspace/symbol", params.clone()).await?;
    }
    let mut hits = Vec::new();
    for sym in res.as_array().into_iter().flatten() {
        let Some(name) = sym.get("name").and_then(|n| n.as_str()) else {
            continue;
        };
        let context = format!("workspace symbol `{name}`");
        let Some(uri) = sym.pointer("/location/uri").and_then(|u| u.as_str()) else {
            if bare_symbol_name(name).eq_ignore_ascii_case(bare_symbol_name(query)) {
                return Err(anyhow::Error::new(MalformedLspCoordinate(format!(
                    "malformed LSP {context}: missing location URI"
                ))));
            }
            continue;
        };
        let Some(path) = Url::parse(uri).ok().and_then(|u| u.to_file_path().ok()) else {
            if bare_symbol_name(name).eq_ignore_ascii_case(bare_symbol_name(query)) {
                return Err(anyhow::Error::new(MalformedLspCoordinate(format!(
                    "malformed LSP {context}: invalid location URI"
                ))));
            }
            continue;
        };
        let Some(start) = sym.pointer("/location/range/start") else {
            if bare_symbol_name(name).eq_ignore_ascii_case(bare_symbol_name(query)) {
                return Err(anyhow::Error::new(MalformedLspCoordinate(format!(
                    "malformed LSP {context}: missing location range start"
                ))));
            }
            continue;
        };
        let matching = bare_symbol_name(name).eq_ignore_ascii_case(bare_symbol_name(query));
        let (line, col) = match lsp_position(start, &context) {
            Ok(position) => position,
            Err(error) if matching => return Err(error),
            Err(_) => continue,
        };
        hits.push(SymbolHit {
            path,
            name: name.to_string(),
            kind: symbol_kind_name(sym.get("kind").and_then(|k| k.as_u64()).unwrap_or(0)),
            container: sym
                .get("containerName")
                .and_then(|c| c.as_str())
                .filter(|c| !c.is_empty())
                .map(str::to_string),
            line,
            col,
        });
        if hits.len() >= limit {
            break;
        }
    }
    Ok(hits)
}

/// Resolves a (possibly qualified) symbol name to one position. Only a hit whose name is the
/// requested name (ignoring ASCII case and the server's decoration, see `bare_symbol_name`)
/// is a candidate: the index also returns prefix and fuzzy matches, and acting on one of those
/// renames or deletes a symbol the caller never named (#253). Qualifiers (`Type::name`,
/// `pkg.Func`, `Class.method`) are matched against the hit's container, `hint` (a file or
/// directory) prefers hits under it. A member the index does not list, such as a Rust struct
/// field, is found through its type's outline. A tie between different locations is an error
/// listing the candidates; no match is an error listing the closest names.
pub async fn resolve_symbol(
    remote: SocketAddr,
    root: &Path,
    symbol: &str,
    hint: Option<&Path>,
) -> Result<SymbolHit> {
    let parts: Vec<&str> = symbol
        .split(['.', ':', '#', '/'])
        .map(|p| p.trim().trim_end_matches("()"))
        .filter(|p| !p.is_empty())
        .collect();
    let name = bare_symbol_name(parts.last().copied().unwrap_or(symbol));
    let qualifier = parts.len().checked_sub(2).map(|i| parts[i]);
    // Every qualifying segment, for a path (`crate::module::item`) the hit's file spells out.
    let qualifiers = &parts[..parts.len().saturating_sub(1)];
    let exact: Vec<SymbolHit> = if qualifier.is_some() {
        // Fast-path: When looking up a qualified symbol (`Type::member`), resolve via the
        // owning type's outline first. The type itself (`qualifiers.last()`) is almost always
        // unique or has very few candidates, whereas common method names (`new`, `get`, `run`, `init`)
        // return 200+ unrelated candidates across the entire workspace and lead to expensive scans.
        let members = type_members(remote, root, qualifiers, name, hint)
            .await
            .unwrap_or_default();
        if !members.is_empty() {
            members
        } else {
            let hits = symbol_search_across_projects(remote, root, name, hint, 200).await?;
            let (exact, others): (Vec<SymbolHit>, Vec<SymbolHit>) = hits
                .into_iter()
                .partition(|hit| bare_symbol_name(&hit.name).eq_ignore_ascii_case(name));
            let eligible: Vec<SymbolHit> = exact
                .into_iter()
                .filter(|hit| qualifier_matches(root, hit, qualifiers))
                .collect();
            if !eligible.is_empty() {
                eligible
            } else {
                anyhow::bail!("{}", no_symbol_message(symbol, name, &others));
            }
        }
    } else {
        let hits = symbol_search_across_projects(remote, root, name, hint, 200).await?;
        let (exact, others): (Vec<SymbolHit>, Vec<SymbolHit>) = hits
            .into_iter()
            .partition(|hit| bare_symbol_name(&hit.name).eq_ignore_ascii_case(name));
        if exact.is_empty() {
            let members = unindexed_members(remote, root, name, hint).await?;
            if !members.is_empty() {
                members
            } else {
                let unindexed_hits = find_unindexed_declarations(root, name, hint).await;
                if !unindexed_hits.is_empty() {
                    unindexed_hits
                } else {
                    let unindexed = unindexed_declarations(remote, root, name).await;
                    anyhow::bail!("{}{unindexed}", no_symbol_message(symbol, name, &others));
                }
            }
        } else {
            exact
        }
    };
    let remote_texts = remote_sources(remote, &exact).await;
    // An explicit owner is not enough when the index or outline is stale. Qualified lookup
    // must point at the requested name in readable source, not merely rank that hit lower.
    let exact = if qualifier.is_some() {
        let verified: Vec<SymbolHit> = exact
            .into_iter()
            .filter(|hit| {
                identifier_at(&hit.path, &remote_texts, hit.line, hit.col, &hit.name)
                    || identifier_at(
                        &hit.path,
                        &remote_texts,
                        hit.line,
                        hit.col,
                        bare_symbol_name(&hit.name),
                    )
            })
            .collect();
        anyhow::ensure!(
            !verified.is_empty(),
            "no verified symbol named `{symbol}`: the index or outline positions do not match readable current source; refresh the project index or supply a current source position"
        );
        verified
    } else {
        exact
    };
    let hint_str = hint.map(|h| h.to_string_lossy().into_owned());
    let mut scored: Vec<(i32, SymbolHit)> = exact
        .into_iter()
        .map(|hit| {
            let bare = bare_symbol_name(&hit.name);
            let mut score = if bare == name { 100 } else { 60 };
            if qualifier.is_some() {
                // Qualified hits already passed `qualifier_matches`; the score is only for
                // ordinary tie-breakers such as a caller's path hint.
            } else if hit.kind == "EnumMember" {
                // A bare name is the type's, not an enum's variant of the same name, which Rust
                // reaches as `Enum::Variant` (#325).
                score -= 20;
            }
            if let Some(h) = &hint_str {
                let p = hit.path.to_string_lossy();
                if *p == **h || p.starts_with(h.as_str()) {
                    score += 30;
                }
            }
            // The checkout's own symbol before a same-named one of the standard library or a
            // dependency, which gopls lists alongside (#345).
            if hit.path.starts_with(root) {
                score += 5;
            }
            // An index can be stale or (clangd) point the right range at the wrong file:
            // the name must actually be at that position. A server that decorates the name
            // (`Type.name`) may point at the bare name, so either spelling counts.
            if !identifier_at(&hit.path, &remote_texts, hit.line, hit.col, &hit.name)
                && !identifier_at(&hit.path, &remote_texts, hit.line, hit.col, bare)
            {
                score -= 1000;
            }
            (score, hit)
        })
        .collect();
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.path.cmp(&b.1.path))
            .then_with(|| a.1.line.cmp(&b.1.line))
    });
    let Some(first) = scored.first() else {
        anyhow::bail!("no symbol named `{symbol}` matches criteria");
    };
    let best = first.0;
    let ties: Vec<&SymbolHit> = scored
        .iter()
        .filter(|(s, _)| *s == best)
        .map(|(_, h)| h)
        .collect();
    // A re-export (`pub use sync::scan;`) is listed by the index next to the definition it
    // names. It is the same symbol, not a second candidate, and the answer is the definition.
    let definitions: Vec<&SymbolHit> = ties
        .iter()
        .copied()
        .filter(|h| !is_use_declaration(&h.path, &remote_texts, h.line))
        .collect();
    let ties = if definitions.is_empty() {
        ties
    } else {
        definitions
    };
    // So is a Swift `extension` of a type, listed under the type's name: the type's own
    // declaration is the answer (#358).
    let declarations: Vec<&SymbolHit> = ties
        .iter()
        .copied()
        .filter(|h| !is_extension_declaration(&h.path, &remote_texts, h.line))
        .collect();
    let ties = if declarations.is_empty() {
        ties
    } else {
        declarations
    };
    single_candidate(root, symbol, &ties)
}

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

/// The Levenshtein distance between two names, counted in characters.
pub(crate) fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = if ca == *cb {
                diagonal
            } else {
                1 + diagonal.min(above).min(row[j])
            };
            diagonal = above;
        }
    }
    row[b.len()]
}

pub(crate) const TYPE_MEMBERS_BUDGET: std::time::Duration = std::time::Duration::from_secs(12);

pub(crate) fn find_source_members(
    root: &Path,
    path: &Path,
    owner: &[&str],
    member: &str,
) -> Vec<SymbolHit> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let type_name = owner.last().copied().unwrap_or_default();
    if !names_word(&text, member) || (!type_name.is_empty() && !names_word(&text, type_name)) {
        return Vec::new();
    }
    if !type_name.is_empty() && !owner_path_matches(root, path, owner, &[type_name.to_string()]) {
        return Vec::new();
    }
    let mut hits = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if let Some(col) = declared_at(line, member) {
            hits.push(SymbolHit {
                path: path.to_path_buf(),
                name: member.to_string(),
                kind: "Property",
                container: Some(owner.join("::")),
                line: index as u32 + 1,
                col: col as u32 + 1,
            });
        }
    }
    hits
}

/// The members called `member` of the type called `type_name`, read from the outline of each
/// file that declares the type. The type is resolved the way a symbol is, by its exact name;
/// with a `hint`, members under it are preferred.
pub(crate) async fn type_members(
    remote: SocketAddr,
    root: &Path,
    owner: &[&str],
    member: &str,
    hint: Option<&Path>,
) -> Result<Vec<SymbolHit>> {
    let Some(type_name) = owner.last().copied() else {
        return Ok(Vec::new());
    };
    let deadline = tokio::time::Instant::now() + TYPE_MEMBERS_BUDGET;
    let types = symbol_search_across_projects(remote, root, type_name, hint, 50)
        .await
        .unwrap_or_default();
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
    for hit in &types {
        if bare_symbol_name(&hit.name).eq_ignore_ascii_case(type_name)
            && !is_use_declaration(&hit.path, &RemoteSources::new(), hit.line)
            && !files.contains(&hit.path)
        {
            files.push(hit.path.clone());
        }
    }
    let mut members: Vec<SymbolHit> = Vec::new();
    for file in files.iter().take(8) {
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
        let mut found = Vec::new();
        collect_members(&outline, root, file, owner, member, &[], &mut found)?;
        for (name, kind, line, col) in found {
            let hit = SymbolHit {
                path: file.clone(),
                name,
                kind: symbol_kind_name(kind),
                container: Some(owner.join("::")),
                line,
                col,
            };
            if !members
                .iter()
                .any(|m| m.path == hit.path && m.line == hit.line && m.col == hit.col)
            {
                members.push(hit);
            }
        }
    }
    let mut candidates = Vec::new();
    if members.is_empty() {
        for path in source_files(root).take(1000) {
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            if !files.contains(&path)
                && std::fs::read_to_string(&path)
                    .is_ok_and(|text| names_word(&text, member) && names_word(&text, type_name))
            {
                candidates.push(path);
                if candidates.len() >= 4 {
                    break;
                }
            }
        }
        for file in &candidates {
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
            let mut found = Vec::new();
            collect_members(&outline, root, file, owner, member, &[], &mut found)?;
            for (name, kind, line, col) in found {
                let hit = SymbolHit {
                    path: file.clone(),
                    name,
                    kind: symbol_kind_name(kind),
                    container: Some(owner.join("::")),
                    line,
                    col,
                };
                if !members
                    .iter()
                    .any(|m| m.path == hit.path && m.line == hit.line && m.col == hit.col)
                {
                    members.push(hit);
                }
            }
        }
    }
    if members.is_empty() {
        for file in files.iter().chain(&candidates) {
            for hit in find_source_members(root, file, owner, member) {
                if !members
                    .iter()
                    .any(|m| m.path == hit.path && m.line == hit.line && m.col == hit.col)
                {
                    members.push(hit);
                }
            }
        }
    }
    if let Some(h) = hint {
        let h_abs = if h.is_absolute() {
            h.to_path_buf()
        } else {
            root.join(h)
        };
        let under_hint =
            |m: &SymbolHit| m.path == h_abs || m.path.starts_with(&h_abs) || m.path.ends_with(h);
        if members.iter().any(under_hint) {
            members.retain(under_hint);
        }
    }
    Ok(members)
}

/// Walks a `textDocument/documentSymbol` answer for the members called `member` of the type
/// called `type_name`, as (name, LSP kind, 1-based line, 1-based column) of the member's name.
/// A nested answer lists them as children of the type, or of an `impl` block for it (that is
/// where rust-analyzer puts methods); a flat one, as the gateway's own engine answers, names the
/// parent in `containerName`, innermost last after ` > `.
pub(crate) fn collect_members(
    symbols: &serde_json::Value,
    root: &Path,
    path: &Path,
    owner: &[&str],
    member: &str,
    ancestors: &[String],
    out: &mut Vec<(String, u64, u32, u32)>,
) -> Result<()> {
    let type_name = owner.last().copied().unwrap_or_default();
    let is_member = |sym: &serde_json::Value| {
        let name = sym.get("name").and_then(|n| n.as_str()).unwrap_or("");
        bare_symbol_name(name).eq_ignore_ascii_case(member)
    };
    for sym in symbols.as_array().into_iter().flatten() {
        let parent = sym.get("containerName").and_then(|c| c.as_str());
        if is_member(sym)
            && parent.is_some_and(|parent| {
                owner_path_matches(root, path, owner, &owner_segments(parent))
            })
        {
            out.push(member_at(sym)?);
        }
        let Some(children) = sym.get("children") else {
            continue;
        };
        let name = sym.get("name").and_then(|n| n.as_str()).unwrap_or("");
        let mut declared = ancestors.to_vec();
        declared.extend(owner_segments(name));
        if names_type(name, type_name) && owner_path_matches(root, path, owner, &declared) {
            for child in children.as_array().into_iter().flatten() {
                if is_member(child) {
                    out.push(member_at(child)?);
                }
            }
        }
        let kind = sym.get("kind").and_then(|kind| kind.as_u64()).unwrap_or(0);
        let nested_ancestors = if matches!(kind, 2..=5 | 10 | 11 | 23) {
            declared
        } else {
            ancestors.to_vec()
        };
        collect_members(children, root, path, owner, member, &nested_ancestors, out)?;
    }
    Ok(())
}

/// A document symbol as (name, kind, line, column) of its name, 1-based: the selection range
/// when there is one, which is the name rather than the doc comment the range starts at.
pub(crate) fn member_at(sym: &serde_json::Value) -> Result<(String, u64, u32, u32)> {
    let start = sym
        .pointer("/selectionRange/start")
        .or_else(|| sym.pointer("/range/start"))
        .or_else(|| sym.pointer("/location/range/start"))
        .ok_or_else(|| {
            anyhow::Error::new(MalformedLspCoordinate(
                "malformed LSP member symbol: missing selection range start".to_string(),
            ))
        })?;
    let name = sym
        .get("name")
        .and_then(|name| name.as_str())
        .ok_or_else(|| {
            anyhow::Error::new(MalformedLspCoordinate(
                "malformed LSP member symbol: missing name".to_string(),
            ))
        })?;
    let (line, col) = lsp_position(start, &format!("member symbol `{name}`"))?;
    Ok((
        name.to_string(),
        sym.get("kind").and_then(|k| k.as_u64()).unwrap_or(0),
        line,
        col,
    ))
}

/// Whether an outline label names the type `type_name`: the type itself, or an `impl` block
/// for it (`impl Type`, `impl<T> Type<T>`, `impl Trait for Type`), whose members are the
/// type's too.
pub(crate) fn names_type(label: &str, type_name: &str) -> bool {
    owner_segments(label).last().is_some_and(|target| {
        target
            .replace('-', "_")
            .eq_ignore_ascii_case(&type_name.replace('-', "_"))
    })
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

/// Directories no project's sources live in: dependencies, build output, virtual environments.
pub(crate) const SKIPPED_DIRS: &[&str] = &[
    "node_modules",
    "target",
    ".git",
    "build",
    "dist",
    ".venv",
    "venv",
    "__pycache__",
    ".build",
    "vendor",
    "Pods",
    "DerivedData",
];

/// The most nested projects a name search asks besides the checkout's own.
pub(crate) const MAX_NESTED_PROJECTS: usize = 6;

/// The most time allowed for cross-project symbol search before returning accumulated hits (#829).
pub(crate) const SYMBOL_SEARCH_BUDGET: std::time::Duration = std::time::Duration::from_secs(30);

/// `workspace/symbol` in the checkout's project and, when that knows no symbol of the name and
/// no hint names a project, in the checkout's nested projects of other languages too (#318):
/// a Swift file in a Go module is in no index gopls keeps.
pub(crate) async fn symbol_search_across_projects(
    remote: SocketAddr,
    root: &Path,
    query: &str,
    hint: Option<&Path>,
    limit: usize,
) -> Result<Vec<SymbolHit>> {
    let deadline = tokio::time::Instant::now() + SYMBOL_SEARCH_BUDGET;
    let mut hits = match tokio::time::timeout_at(
        deadline,
        workspace_symbol_search(remote, root, query, hint, limit),
    )
    .await
    {
        Ok(result) => result?,
        Err(_) => {
            tracing::warn!(
                query,
                "primary workspace symbol search reached budget; returning no hits"
            );
            return Ok(Vec::new());
        }
    };
    let name = bare_symbol_name(query);
    let named = |hits: &[SymbolHit]| {
        hits.iter()
            .any(|hit| bare_symbol_name(&hit.name).eq_ignore_ascii_case(name))
    };
    if named(&hits) {
        return Ok(hits);
    }
    if let Some(hint) = hint {
        // The project a path names may keep no index (sourcekit-lsp before a build): the
        // outlines of its files that name the symbol still find it (#358).
        if let (_, Some(engine)) = crate::sync::engine_project(root, hint) {
            let node = match tokio::time::timeout_at(
                deadline,
                crate::cluster::route_for_path(remote, root, hint.to_str()),
            )
            .await
            {
                Ok(Ok(n)) => n,
                Ok(Err(_)) => remote,
                Err(_) => return Ok(hits),
            };
            let files = if hint.is_file() {
                read_name_scan_text(hint)
                    .is_some_and(|text| names_word(&text, name))
                    .then(|| hint.to_path_buf())
                    .into_iter()
                    .collect()
            } else {
                files_naming(hint, engine, name, deadline)
            };
            if !files.is_empty()
                && let Ok(Ok(decls)) =
                    tokio::time::timeout_at(deadline, declarations_in(node, root, &files, name))
                        .await
            {
                hits.extend(decls);
            }
        }
        return Ok(hits);
    }
    // The projects whose sources name the symbol first, then the others a walk meets (#358).
    // If the root project already returned relevant matches (prefix, word-boundary, substring),
    // and no nested project's sources explicitly name the query, avoid falling back to
    // arbitrary nested project anchors which can cause runaway timeouts (#829).
    let has_relevant_hits = hits.iter().any(|hit| match_rank(&hit.name, query) < 4);
    let mut anchors = projects_naming(root, name, deadline);
    if anchors.is_empty() {
        if has_relevant_hits {
            return Ok(hits);
        }
        for anchor in nested_project_anchors(root, deadline) {
            if anchors.len() >= MAX_NESTED_PROJECTS {
                break;
            }
            if !anchors
                .iter()
                .any(|(_, subpath, engine)| *subpath == anchor.1 && *engine == anchor.2)
            {
                anchors.push(anchor);
            }
        }
    }
    for (anchor, subpath, engine) in anchors {
        if tokio::time::Instant::now() >= deadline {
            tracing::warn!(
                query,
                "symbol search across projects reached query budget; returning accumulated hits"
            );
            break;
        }
        // Its own node: the checkout's may not serve its language (a Swift file needs a macOS
        // node, a Go module is placed on Linux).
        let node = match tokio::time::timeout_at(
            deadline,
            crate::cluster::route_for_path(remote, root, anchor.to_str()),
        )
        .await
        {
            Ok(Ok(n)) => n,
            Ok(Err(_)) => remote,
            Err(_) => {
                tracing::warn!(
                    query,
                    "symbol search route_for_path reached budget; returning accumulated hits"
                );
                break;
            }
        };

        if tokio::time::Instant::now() >= deadline {
            break;
        }

        let search_fut = workspace_symbol_search_auxiliary(node, root, query, Some(&anchor), limit);
        let found = match tokio::time::timeout_at(deadline, search_fut).await {
            Ok(Ok(found)) => found,
            Ok(Err(err)) => {
                tracing::debug!(
                    anchor = %anchor.display(),
                    error = %format!("{err:#}"),
                    "a nested project's symbol search failed"
                );
                if is_malformed_lsp_coordinate(&err) {
                    return Err(err);
                }
                Vec::new()
            }
            Err(_) => {
                tracing::warn!(
                    query,
                    "nested workspace_symbol_search reached budget; returning accumulated hits"
                );
                break;
            }
        };
        let named_here = found
            .iter()
            .any(|hit| bare_symbol_name(&hit.name).eq_ignore_ascii_case(name));
        hits.extend(found);
        if !named_here && tokio::time::Instant::now() < deadline {
            // sourcekit-lsp has no index for Swift files a package does not build, and answers
            // `workspace/symbol` with nothing: their outlines still name what they declare.
            let files = files_naming(&root.join(&subpath), engine, name, deadline);
            if !files.is_empty() {
                match tokio::time::timeout_at(deadline, declarations_in(node, root, &files, name))
                    .await
                {
                    Ok(Ok(decls)) => hits.extend(decls),
                    Ok(Err(e)) => return Err(e),
                    Err(_) => {
                        tracing::warn!(
                            query,
                            "declarations_in reached budget; returning accumulated hits"
                        );
                        break;
                    }
                }
            }
        }
        if named(&hits) {
            break;
        }
    }
    Ok(hits)
}

/// The most files of a project read for the declarations of a name its server has no index of.
pub(crate) const MAX_OUTLINED_FILES: usize = 8;

/// Files of `engine`'s language under `dir` whose text has `name` as a word, prioritizing files
/// that declare `name`.
pub(crate) fn files_naming(
    dir: &Path,
    engine: &str,
    name: &str,
    deadline: tokio::time::Instant,
) -> Vec<std::path::PathBuf> {
    let mut files: Vec<(bool, std::path::PathBuf)> = Vec::new();
    for path in source_files(dir).filter(|path| crate::sync::engine_for_file(path) == Some(engine))
    {
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        let Some(text) = read_name_scan_text(&path) else {
            continue;
        };
        if !names_word(&text, name) {
            continue;
        }
        let has_decl = text.lines().any(|line| declared_at(line, name).is_some());
        files.push((has_decl, path));
        if files.len() >= MAX_OUTLINED_FILES * 2 {
            break;
        }
    }
    files.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    files
        .into_iter()
        .map(|(_, p)| p)
        .take(MAX_OUTLINED_FILES)
        .collect()
}

/// Whether `text` has `name` as a whole word.
pub(crate) fn names_word(text: &str, name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    text.split(|c: char| !is_word(c))
        .any(|word| word.eq_ignore_ascii_case(name))
}

/// Maximum bytes read from any one source while searching for names synchronously.
pub(crate) const MAX_NAME_SCAN_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// Reads only the prefix needed for bounded symbol-name discovery. Large generated sources do
/// not get to exceed the overall search budget through one unbounded synchronous read.
pub(crate) fn read_name_scan_text(path: &Path) -> Option<String> {
    use std::io::Read;

    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(MAX_NAME_SCAN_FILE_BYTES)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// How far below a directory a search of its sources looks.
pub(crate) const MAX_SEARCH_DEPTH: usize = 6;

/// The files under `dir` a search of the checkout looks at, in path order: what git does not
/// ignore, hidden directories and [`SKIPPED_DIRS`] left out, at most [`MAX_SEARCH_DEPTH`] levels
/// down. A dependency's build tree next to its sources is an ignored part of the checkout and
/// stays out, where a plain walk spent the search on it (#358).
pub(crate) fn source_files(dir: &Path) -> impl Iterator<Item = std::path::PathBuf> {
    ignore::WalkBuilder::new(dir)
        .max_depth(Some(MAX_SEARCH_DEPTH))
        .sort_by_file_name(|a, b| a.cmp(b))
        .filter_entry(|entry| {
            entry.depth() == 0
                || !entry.file_type().is_some_and(|t| t.is_dir())
                || !SKIPPED_DIRS.contains(&entry.file_name().to_string_lossy().as_ref())
        })
        .build()
        .flatten()
        .filter(|entry| entry.file_type().is_some_and(|t| t.is_file()))
        .map(ignore::DirEntry::into_path)
}

/// The most source files a name search reads to find the nested projects that mention it.
pub(crate) const MAX_SCANNED_FILES: usize = 5000;

/// One file of each nested project whose sources name `name`, in path order: the projects to
/// ask first (#358). A walk that takes the first projects it meets spent every slot on a C++
/// dependency and loose scripts while the declaration sat in a Swift package after them.
pub(crate) fn projects_naming(
    root: &Path,
    name: &str,
    deadline: tokio::time::Instant,
) -> Vec<(std::path::PathBuf, String, &'static str)> {
    let root_engine = crate::sync::expected_engine(root);
    let mut projects = std::collections::HashSet::new();
    let mut anchors = Vec::new();
    for path in source_files(root)
        .filter(|path| crate::sync::engine_for_file(path).is_some_and(|e| Some(e) != root_engine))
        .take(MAX_SCANNED_FILES)
    {
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        if !read_name_scan_text(&path).is_some_and(|text| names_word(&text, name)) {
            continue;
        }
        if let (Some(subpath), Some(engine)) = crate::sync::engine_project(root, &path)
            && projects.insert((subpath.clone(), engine))
        {
            anchors.push((path, subpath, engine));
            if anchors.len() >= MAX_NESTED_PROJECTS {
                break;
            }
        }
    }
    anchors
}

/// The declarations called `name` in the outlines of `files`.
pub(crate) async fn declarations_in(
    remote: SocketAddr,
    root: &Path,
    files: &[std::path::PathBuf],
    name: &str,
) -> Result<Vec<SymbolHit>> {
    let mut hits = Vec::new();
    for file in files {
        let Ok(uri) = Url::from_file_path(file) else {
            continue;
        };
        let params = serde_json::json!({ "textDocument": { "uri": uri.to_string() } });
        let Ok(outline) =
            execute_lsp_query(remote, root, file, "textDocument/documentSymbol", params).await
        else {
            continue;
        };
        collect_named(&outline, name, None, file, &mut hits)?;
        if hits
            .iter()
            .any(|hit| bare_symbol_name(&hit.name).eq_ignore_ascii_case(name))
        {
            break;
        }
    }
    Ok(hits)
}

/// Walks a `textDocument/documentSymbol` answer, nested or flat, for the symbols called `name`.
pub(crate) fn collect_named(
    symbols: &serde_json::Value,
    name: &str,
    parent: Option<&str>,
    file: &Path,
    out: &mut Vec<SymbolHit>,
) -> Result<()> {
    for symbol in symbols.as_array().into_iter().flatten() {
        let own = symbol.get("name").and_then(|n| n.as_str()).unwrap_or("");
        if bare_symbol_name(own).eq_ignore_ascii_case(name) {
            let start = symbol
                .pointer("/selectionRange/start")
                .or_else(|| symbol.pointer("/location/range/start"));
            if let Some(start) = start {
                let (line, col) = lsp_position(start, &format!("named declaration `{own}`"))?;
                out.push(SymbolHit {
                    path: file.to_path_buf(),
                    name: own.to_string(),
                    kind: symbol_kind_name(
                        symbol.get("kind").and_then(|k| k.as_u64()).unwrap_or(0),
                    ),
                    container: parent.map(str::to_string).or_else(|| {
                        symbol
                            .get("containerName")
                            .and_then(|c| c.as_str())
                            .filter(|c| !c.is_empty())
                            .map(str::to_string)
                    }),
                    line,
                    col,
                });
            } else {
                return Err(anyhow::Error::new(MalformedLspCoordinate(format!(
                    "malformed LSP named declaration `{own}`: missing selection range start"
                ))));
            }
        }
        if let Some(children) = symbol.get("children") {
            collect_named(children, name, Some(own), file, out)?;
        }
    }
    Ok(())
}

/// One source file of each project in the checkout besides the root's, with the project's
/// directory (relative) and engine: a nested project of another language, or a loose file of
/// one, as `engine_project` places them (#247, #318).
pub(crate) fn nested_project_anchors(
    root: &Path,
    deadline: tokio::time::Instant,
) -> Vec<(std::path::PathBuf, String, &'static str)> {
    let mut seen_dirs = std::collections::HashSet::new();
    let mut projects = std::collections::HashSet::new();
    let mut anchors = Vec::new();
    for path in source_files(root) {
        if tokio::time::Instant::now() >= deadline {
            return anchors;
        }
        let Some(engine) = crate::sync::engine_for_file(&path) else {
            continue;
        };
        // One look per directory and language: its files belong to the same project.
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        if !seen_dirs.insert((dir, engine)) {
            continue;
        }
        if let (Some(subpath), Some(engine)) = crate::sync::engine_project(root, &path)
            && projects.insert((subpath.clone(), engine))
        {
            anchors.push((path, subpath, engine));
            if anchors.len() >= MAX_NESTED_PROJECTS {
                return anchors;
            }
        }
    }
    anchors
}

/// A source file of the project at `dir` in its main language (shortest path under `src`
/// first), used to make an LSP server load the project before a workspace-level query.
pub(crate) fn representative_source_file(dir: &Path) -> Option<std::path::PathBuf> {
    let (_, language) = crate::sync::engine_project(dir, dir);
    let exts: &[&str] = match language? {
        "rust" => &["rs"],
        "go" => &["go"],
        "cpp" => &["cpp", "cc", "cxx", "c", "hpp", "h"],
        "python" => &["py"],
        "typescript" => &["ts", "tsx", "mts", "js", "jsx"],
        "swift" => &["swift"],
        _ => return None,
    };
    let mut best: Option<(usize, usize, std::path::PathBuf)> = None;
    let mut stack = vec![(dir.to_path_buf(), 0usize)];
    while let Some((d, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                if depth < 4 && !SKIPPED_DIRS.contains(&name.as_str()) && !name.starts_with('.') {
                    stack.push((path, depth + 1));
                }
                continue;
            }
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !exts.contains(&ext) || name.ends_with(".d.ts") {
                continue;
            }
            let rel = path
                .strip_prefix(dir)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned();
            let outside_src = usize::from(!(rel.starts_with("src/") || rel.starts_with("lib/")));
            let is_test = usize::from(rel.contains("test") || rel.contains("spec"));
            let key = (outside_src + is_test, rel.len());
            // A file of a nested project that is its own (another language, or a crate the
            // root workspace leaves out) would open the session in that project (#335).
            if best.as_ref().is_none_or(|(a, b, _)| key < (*a, *b))
                && crate::sync::engine_project(dir, &path).0.is_none()
            {
                best = Some((key.0, key.1, path));
            }
        }
    }
    best.map(|(_, _, p)| p)
}

/// Whether the 1-based `line` of `path` is in a `use` declaration (`use a::b;`, `pub use`,
/// `pub(crate) use`, and any line of one that spans lines, `pub use m::{\n    A,\n    B,\n};`):
/// what the workspace index lists for a re-export, next to the definition it names (#128, #225).
/// Texts of files a symbol hit points at that are not on this machine, keyed by path.
type RemoteSources = std::collections::HashMap<std::path::PathBuf, String>;

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
    row[start..].starts_with(bare)
}
