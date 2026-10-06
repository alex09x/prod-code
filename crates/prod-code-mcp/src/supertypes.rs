/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Supertypes (roadmap 7.5): what a type implements, and what a trait requires. The other
//! direction, what implements a trait, is `code_implementations`.
//!
//! rust-analyzer has no LSP type hierarchy, so for Rust the answer is read from what it does
//! have. For a trait, the supertraits are the bounds after the colon in its own header. For a
//! type, the derived traits are read from the `#[derive(…)]` attributes above it, and the
//! written ones from its implementations (`textDocument/implementation` on the type): an
//! `impl Trait for Type` header gives `Trait`, and an inherent `impl Type` is not a supertype.
//! rust-analyzer reports a derive among the implementations too, at the attribute for a
//! built-in derive and at the type's own name for a macro such as serde's, which is why derives
//! are read from the attributes instead.
//!
//! The other languages' servers are asked for their own type hierarchy
//! (`textDocument/prepareTypeHierarchy`, then `typeHierarchy/supertypes`). A server without one
//! gets that said instead of an empty list.

use crate::tools::execute_lsp_query;
use anyhow::Result;
use std::collections::HashSet;
use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::pin::Pin;

/// The deepest type hierarchy asked for; a larger depth is read as this.
pub const MAX_DEPTH: usize = 6;

/// One supertype, and where the relation is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Supertype {
    pub name: String,
    /// Written as a derive rather than an impl block.
    pub derived: bool,
    /// Where the impl, the derive or the supertype itself is (1-based); `None` for a supertrait
    /// read from a header.
    pub at: Option<(PathBuf, u32, u32)>,
    /// Supertypes of this supertype (when depth > 1).
    pub children: Vec<Supertype>,
    /// Already shown higher in the hierarchy (cycle avoidance).
    pub repeated: bool,
}

impl Supertype {
    pub fn new(name: impl Into<String>, derived: bool, at: Option<(PathBuf, u32, u32)>) -> Self {
        Self {
            name: name.into(),
            derived,
            at,
            children: Vec::new(),
            repeated: false,
        }
    }
}

/// What the supertypes are of, which decides how they are named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A Rust type: the traits it implements.
    Type,
    /// A Rust trait: its supertraits.
    Trait,
    /// Another language, answered by its server's type hierarchy.
    Other,
}

/// The supertypes of one type or trait.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Supertypes {
    pub of: String,
    pub kind: Kind,
    pub list: Vec<Supertype>,
    pub depth: usize,
    /// Why there is no answer: the language server has no type hierarchy.
    pub unsupported: Option<String>,
}

impl Supertypes {
    /// Every supertype in the hierarchy, at every level.
    pub fn count(&self) -> usize {
        fn count_nodes(nodes: &[Supertype]) -> usize {
            nodes.iter().map(|n| 1 + count_nodes(&n.children)).sum()
        }
        count_nodes(&self.list)
    }

    pub fn render(&self, root: &Path) -> String {
        if let Some(why) = &self.unsupported {
            return why.clone();
        }
        let (verb, noun) = match self.kind {
            Kind::Type => ("implements", "trait"),
            Kind::Trait => ("requires", "supertrait"),
            Kind::Other => ("has", "supertype"),
        };
        if self.list.is_empty() {
            return format!("`{}` {verb} no {noun}.", self.of);
        }
        let mut out = if self.depth > 1 {
            format!(
                "`{}` {verb} {} {noun}(s), {} in all to depth {}:",
                self.of,
                self.list.len(),
                self.count(),
                self.depth
            )
        } else {
            format!("`{}` {verb} {} {noun}(s):", self.of, self.list.len())
        };
        fn render_nodes(out: &mut String, nodes: &[Supertype], root: &Path, indent: usize) {
            for s in nodes {
                out.push('\n');
                out.push_str(&"  ".repeat(indent));
                out.push_str(&format!("• {}", s.name));
                if s.derived {
                    out.push_str("  (derived)");
                }
                if let Some((path, line, col)) = &s.at {
                    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
                    let canonical_path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
                    let shown = path
                        .strip_prefix(root)
                        .or_else(|_| canonical_path.strip_prefix(&canonical_root))
                        .or_else(|_| path.strip_prefix(&canonical_root))
                        .or_else(|_| canonical_path.strip_prefix(root))
                        .unwrap_or(path);
                    out.push_str(&format!("  {}:{line}:{col}", shown.display()));
                }
                if s.repeated {
                    out.push_str("  (shown above)");
                } else if !s.children.is_empty() {
                    render_nodes(out, &s.children, root, indent + 1);
                }
            }
        }
        render_nodes(&mut out, &self.list, root, 1);
        out
    }
}

/// The identifier around the 1-based character `col` of `line`.
fn word_at(line: &str, col: u32) -> Option<String> {
    let chars: Vec<char> = line.chars().collect();
    let at = (col as usize).checked_sub(1)?;
    let is_word = |c: &char| c.is_alphanumeric() || *c == '_';
    if !chars.get(at).is_some_and(is_word) {
        return None;
    }
    let start = (0..=at).rev().take_while(|i| is_word(&chars[*i])).last()?;
    let end = (at..chars.len())
        .take_while(|i| is_word(&chars[*i]))
        .last()?
        + 1;
    Some(chars[start..end].iter().collect())
}

/// The text of `lines` from `from` (0-based) up to the first `{` or `;`, on one line.
fn header_from(lines: &[&str], from: usize) -> String {
    let mut header = String::new();
    for line in lines.iter().skip(from) {
        match line.find(['{', ';']) {
            Some(end) => {
                header.push_str(&line[..end]);
                break;
            }
            None => {
                header.push_str(line);
                header.push(' ');
            }
        }
    }
    header
}

/// `text` split at `sep` where it is outside `<…>` and `(…)`.
fn split_top(text: &str, sep: char) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut current = String::new();
    for c in text.chars() {
        match c {
            '<' | '(' => depth += 1,
            '>' | ')' => depth -= 1,
            _ => {}
        }
        if c == sep && depth == 0 {
            parts.push(current.trim().to_string());
            current.clear();
        } else {
            current.push(c);
        }
    }
    parts.push(current.trim().to_string());
    parts.into_iter().filter(|p| !p.is_empty()).collect()
}

/// `text` after a leading `<…>`, with the brackets balanced.
fn skip_generics(text: &str) -> &str {
    let text = text.trim_start();
    if !text.starts_with('<') {
        return text;
    }
    let mut depth = 0;
    for (i, c) in text.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => {
                depth -= 1;
                if depth == 0 {
                    return &text[i + 1..];
                }
            }
            _ => {}
        }
    }
    ""
}

/// The trait an impl header implements: `Default` for `impl Default for Cache`, `Into<u8>` for
/// `impl<T> Into<u8> for Wrapper<T>`; `None` for an inherent `impl Cache`.
pub fn impl_trait(header: &str) -> Option<String> {
    let at = header.find("impl")?;
    let rest = skip_generics(&header[at + 4..]);
    let rest = rest.split(" where ").next().unwrap_or(rest);
    let mut depth = 0i32;
    let chars: Vec<(usize, char)> = rest.char_indices().collect();
    for (n, &(i, c)) in chars.iter().enumerate() {
        match c {
            '<' | '(' => depth += 1,
            '>' | ')' => depth -= 1,
            _ => {}
        }
        if depth == 0 && rest[i..].starts_with(" for ") && n > 0 {
            let name = rest[..i].trim();
            return (!name.is_empty()).then(|| name.to_string());
        }
    }
    None
}

/// The supertraits in a trait header: `Send + Sync` in `pub trait Embed: Send + Sync {`, and
/// the bounds on `Self` in its `where` clause, which the Rust Reference counts as supertraits
/// too (`trait Circle where Self: Shape`, #227).
pub fn supertraits(header: &str) -> Vec<String> {
    let Some(at) = header.find("trait ") else {
        return Vec::new();
    };
    let after = &header[at + 6..];
    let name_end = after
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(after.len());
    let rest = skip_generics(&after[name_end..]).trim_start();
    // `where` as a word, not inside a name such as `Somewhere`.
    let keyword = rest.match_indices("where").map(|(i, _)| i).find(|&i| {
        (i == 0 || rest[..i].ends_with(char::is_whitespace))
            && rest[i + 5..].starts_with(char::is_whitespace)
    });
    let (bounds, clause) = match keyword {
        Some(w) => (&rest[..w], &rest[w + "where".len()..]),
        None => (rest, ""),
    };
    let mut out = bounds
        .trim_start()
        .strip_prefix(':')
        .map(|b| split_top(b, '+'))
        .unwrap_or_default();
    for predicate in split_top(clause, ',') {
        if let Some(on_self) = predicate.strip_prefix("Self")
            && let Some(b) = on_self.trim_start().strip_prefix(':')
        {
            for bound in split_top(b, '+') {
                if !out.contains(&bound) {
                    out.push(bound);
                }
            }
        }
    }
    out
}

/// The traits derived by the `#[derive(…)]` attributes directly above the declaration on line
/// `decl` (0-based), each with its 1-based position. An attribute may span lines.
fn derives_above(lines: &[&str], decl: usize) -> Vec<(String, u32, u32)> {
    if lines.is_empty() || decl == 0 {
        return Vec::new();
    }
    // The attributes and doc comments of this item: up to the end of the one before it.
    let mut start = decl.min(lines.len());
    while start > 0 {
        let above = lines[start - 1].trim();
        if above.is_empty() || above.ends_with('}') || above.ends_with(';') || decl - start >= 40 {
            break;
        }
        start -= 1;
    }
    let mut out = Vec::new();
    let mut inside = false;
    for (n, line) in lines.iter().enumerate().take(decl.min(lines.len())).skip(start) {
        let mut from = 0;
        if !inside {
            match line.find("derive(") {
                Some(at) if line.trim_start().starts_with("#[") || line[..at].contains("#[") => {
                    inside = true;
                    from = at + "derive(".len();
                }
                _ => continue,
            }
        }
        let body = &line[from..];
        let end = body.find(')');
        let names = &body[..end.unwrap_or(body.len())];
        let mut offset = from;
        for part in names.split(',') {
            let name = part.trim();
            if !name.is_empty() {
                let col = line[..offset + part.find(name).unwrap_or(0)]
                    .chars()
                    .count() as u32
                    + 1;
                out.push((name.to_string(), n as u32 + 1, col));
            }
            offset += part.len() + 1;
        }
        if end.is_some() {
            inside = false;
        }
    }
    out
}

/// The line an impl header starts on, when the location on line `at` (0-based) is in one: at
/// most three lines up, for a header broken over lines.
fn impl_header_start(lines: &[&str], at: usize) -> Option<usize> {
    (at.saturating_sub(3)..=at).rev().find(|i| {
        let line = lines[*i].trim_start();
        line.starts_with("impl") || line.starts_with("unsafe impl")
    })
}

/// Is the declaration on this line a trait?
fn is_trait_decl(line: &str) -> bool {
    let mut rest = line.trim_start();
    for prefix in ["pub(crate) ", "pub(super) ", "pub ", "unsafe ", "auto "] {
        rest = rest.strip_prefix(prefix).unwrap_or(rest);
    }
    rest.starts_with("trait ")
}

/// A location from an LSP answer: a `Location` or a `LocationLink`, as (path, 0-based line,
/// 0-based character).
fn location_of(value: &serde_json::Value) -> Option<(PathBuf, u32, u32)> {
    let uri = value
        .get("uri")
        .or_else(|| value.get("targetUri"))?
        .as_str()?;
    let range = value
        .get("range")
        .or_else(|| value.get("targetSelectionRange"))?;
    let at = |k: &str| {
        range
            .pointer(&format!("/start/{k}"))
            .and_then(|v| v.as_u64())
    };
    Some((
        PathBuf::from(crate::remote_fs::uri_to_path(uri)),
        at("line")? as u32,
        at("character")? as u32,
    ))
}

fn locations(value: &serde_json::Value) -> Vec<(PathBuf, u32, u32)> {
    match value {
        serde_json::Value::Array(items) => items.iter().filter_map(location_of).collect(),
        other => location_of(other).into_iter().collect(),
    }
}

fn hierarchy_range(item: &serde_json::Value) -> Option<&serde_json::Value> {
    match item.get("selectionRange") {
        Some(range) => Some(range),
        None => item.get("range"),
    }
}

fn hierarchy_start(item: &serde_json::Value) -> Option<&serde_json::Value> {
    hierarchy_range(item).and_then(|range| range.get("start"))
}

fn lsp_position(position: Option<&serde_json::Value>) -> Option<(u64, u64)> {
    let line = position?.get("line")?.as_u64()?;
    let character = position?.get("character")?.as_u64()?;
    (line < u64::from(u32::MAX) && character < u64::from(u32::MAX))
        .then_some((line, character))
}

fn valid_lsp_position(position: Option<&serde_json::Value>) -> bool {
    lsp_position(position).is_some()
}

fn valid_lsp_range(range: &serde_json::Value) -> bool {
    matches!(
        (lsp_position(range.get("start")), lsp_position(range.get("end"))),
        (Some(start), Some(end)) if start <= end
    )
}

fn validate_type_hierarchy_item(item: &serde_json::Value) -> Result<()> {
    anyhow::ensure!(
        item.is_object(),
        "type hierarchy item is not an object: {item}"
    );
    anyhow::ensure!(
        item.get("name").and_then(|n| n.as_str()).is_some(),
        "type hierarchy item has no valid 'name': {item}"
    );
    anyhow::ensure!(
        item.get("uri").and_then(|u| u.as_str()).is_some(),
        "type hierarchy item has no valid 'uri': {item}"
    );
    let has_valid_range = hierarchy_range(item).is_some_and(valid_lsp_range);
    anyhow::ensure!(
        has_valid_range,
        "type hierarchy item has no valid 'selectionRange' or 'range': {item}"
    );
    Ok(())
}

/// The supertypes of the type or trait at the 1-based `line`:`character` of `file`.
pub async fn supertypes(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    character: u32,
    depth: usize,
) -> Result<Supertypes> {
    let depth = depth.clamp(1, MAX_DEPTH);
    let uri = url::Url::from_file_path(file)
        .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {file:?}"))?
        .to_string();
    let position = serde_json::json!({
        "textDocument": { "uri": uri },
        "position": { "line": line.saturating_sub(1), "character": character.saturating_sub(1) },
    });
    if file.extension().is_some_and(|e| e == "rs") {
        return rust_supertypes(remote, root, file, line, character, position, depth).await;
    }
    let prepared = execute_lsp_query(
        remote,
        root,
        file,
        "textDocument/prepareTypeHierarchy",
        position,
    )
    .await?;
    let prepared_array = match &prepared {
        serde_json::Value::Null => {
            return Ok(Supertypes {
                of: format!("{}:{line}:{character}", file.display()),
                kind: Kind::Other,
                list: Vec::new(),
                depth,
                unsupported: Some(format!(
                    "No type hierarchy at {}:{line}:{character}: the language server answered none (not every server has one).",
                    file.display()
                )),
            });
        }
        serde_json::Value::Array(a) => a,
        other => anyhow::bail!(
            "the analyzer's prepareTypeHierarchy answer is not an array or null: {other}"
        ),
    };
    if prepared_array.is_empty() {
        return Ok(Supertypes {
            of: format!("{}:{line}:{character}", file.display()),
            kind: Kind::Other,
            list: Vec::new(),
            depth,
            unsupported: Some(format!(
                "No type hierarchy at {}:{line}:{character}: the language server answered none (not every server has one).",
                file.display()
            )),
        });
    }
    for item in prepared_array {
        validate_type_hierarchy_item(item)?;
    }
    let item = &prepared_array[0];
    let mut seen = HashSet::new();
    let root_item_start = hierarchy_start(item);
    let root_line = root_item_start.and_then(|s| s.get("line")).and_then(|v| v.as_u64()).unwrap_or(0);
    let root_col = root_item_start.and_then(|s| s.get("character")).and_then(|v| v.as_u64()).unwrap_or(0);
    let root_uri = item.get("uri").and_then(|u| u.as_str()).unwrap_or("");
    seen.insert(format!("{root_uri}:{root_line}:{root_col}"));
    let list = expand_lsp_supertypes(remote, root, file, item.clone(), 1, depth, &mut seen).await?;
    Ok(Supertypes {
        of: item
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("?")
            .to_string(),
        kind: Kind::Other,
        list,
        depth,
        unsupported: None,
    })
}

fn expand_lsp_supertypes<'a>(
    remote: SocketAddr,
    root: &'a Path,
    file: &'a Path,
    item: serde_json::Value,
    level: usize,
    max_depth: usize,
    seen: &'a mut HashSet<String>,
) -> Pin<Box<dyn Future<Output = Result<Vec<Supertype>>> + Send + 'a>> {
    Box::pin(async move {
        if level > max_depth {
            return Ok(Vec::new());
        }
        let supers = execute_lsp_query(
            remote,
            root,
            file,
            "typeHierarchy/supertypes",
            serde_json::json!({ "item": item }),
        )
        .await?;
        let supers_array = match &supers {
            serde_json::Value::Null => return Ok(Vec::new()),
            serde_json::Value::Array(a) => a,
            other => anyhow::bail!(
                "the analyzer's typeHierarchy/supertypes answer is not an array or null: {other}"
            ),
        };
        let mut list = Vec::with_capacity(supers_array.len());
        for s in supers_array {
            validate_type_hierarchy_item(s)?;
            let name = s.get("name").and_then(|n| n.as_str()).unwrap().to_string();
            let uri = s.get("uri").and_then(|u| u.as_str()).unwrap();
            let start = hierarchy_start(s).unwrap();
            let line = start.get("line").and_then(|v| v.as_u64()).unwrap() as u32 + 1;
            let col = start.get("character").and_then(|v| v.as_u64()).unwrap() as u32 + 1;
            let path = PathBuf::from(crate::remote_fs::uri_to_path(uri));
            let mut node = Supertype::new(name, false, Some((path, line, col)));
            let key = format!("{uri}:{line}:{col}");
            if !seen.insert(key.clone()) {
                node.repeated = true;
            } else if level < max_depth {
                node.children =
                    expand_lsp_supertypes(remote, root, file, s.clone(), level + 1, max_depth, seen)
                        .await?;
            }
            list.push(node);
        }
        Ok(list)
    })
}

fn expand_rust_trait_supertraits<'a>(
    remote: SocketAddr,
    root: &'a Path,
    file: &'a Path,
    trait_name: &'a str,
    ref_file: &'a Path,
    ref_line: u32,
    ref_col: u32,
    level: usize,
    max_depth: usize,
    seen: &'a mut HashSet<String>,
) -> Pin<Box<dyn Future<Output = Vec<Supertype>> + Send + 'a>> {
    Box::pin(async move {
        if level >= max_depth {
            return Vec::new();
        }
        let bare = trait_name.split('<').next().unwrap_or(trait_name).trim();
        if bare.starts_with('\'') || bare.is_empty() {
            return Vec::new();
        }
        let ref_uri = match url::Url::from_file_path(ref_file) {
            Ok(u) => u.to_string(),
            Err(_) => return Vec::new(),
        };
        let def_res = execute_lsp_query(
            remote,
            root,
            ref_file,
            "textDocument/definition",
            serde_json::json!({
                "textDocument": { "uri": ref_uri },
                "position": { "line": ref_line.saturating_sub(1), "character": ref_col.saturating_sub(1) },
            }),
        )
        .await;
        let Some((def_file, def_line, def_col)) = def_res.ok().and_then(|v| locations(&v).into_iter().next()) else {
            return Vec::new();
        };
        let key = format!("{}:{def_line}:{def_col}", def_file.display());
        if !seen.insert(key.clone()) {
            return Vec::new();
        }
        let text = if let Ok((bytes, _)) =
            crate::remote_fs::read_source(remote, root, &def_file.to_string_lossy()).await
        {
            String::from_utf8_lossy(&bytes).into_owned()
        } else {
            std::fs::read_to_string(&def_file).unwrap_or_default()
        };
        let lines: Vec<&str> = text.lines().collect();
        if def_line as usize >= lines.len() {
            return Vec::new();
        }
        let decl = lines[def_line as usize];
        if !is_trait_decl(decl) {
            return Vec::new();
        }
        let sub_names = supertraits(&header_from(&lines, def_line as usize));
        let mut children = Vec::with_capacity(sub_names.len());
        for name in sub_names {
            let bare_sub = name.split('<').next().unwrap_or(&name).trim();
            let mut sub_line = def_line + 1;
            let mut sub_col = def_col + 1;
            for (idx, line_str) in lines.iter().enumerate().skip(def_line as usize) {
                if let Some(pos) = line_str.find(bare_sub) {
                    sub_line = idx as u32 + 1;
                    sub_col = pos as u32 + 1;
                    break;
                }
                if line_str.contains(['{', ';']) {
                    break;
                }
            }
            let mut node = Supertype::new(name.clone(), false, None);
            let child_key = format!("{name}:{sub_line}:{sub_col}");
            if seen.contains(&child_key) {
                node.repeated = true;
            } else if level + 1 < max_depth {
                node.children = expand_rust_trait_supertraits(
                    remote,
                    root,
                    file,
                    &name,
                    &def_file,
                    sub_line,
                    sub_col,
                    level + 1,
                    max_depth,
                    seen,
                )
                .await;
            }
            children.push(node);
        }
        children
    })
}

async fn rust_supertypes(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    character: u32,
    position: serde_json::Value,
    depth: usize,
) -> Result<Supertypes> {
    // The declaration: where the name at the position is defined, or the position itself.
    let definition = execute_lsp_query(remote, root, file, "textDocument/definition", position)
        .await
        .ok()
        .and_then(|v| locations(&v).into_iter().next())
        .unwrap_or((
            file.to_path_buf(),
            line.saturating_sub(1),
            character.saturating_sub(1),
        ));
    let (decl_file, decl_line, decl_col) = definition;
    let text = if let Ok((bytes, _)) =
        crate::remote_fs::read_source(remote, root, &decl_file.to_string_lossy()).await
    {
        String::from_utf8_lossy(&bytes).into_owned()
    } else {
        std::fs::read_to_string(&decl_file).unwrap_or_default()
    };
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return Ok(Supertypes {
            of: "?".to_string(),
            kind: Kind::Other,
            list: Vec::new(),
            depth,
            unsupported: None,
        });
    }
    let decl_idx = (decl_line as usize).min(lines.len() - 1);
    let decl = lines.get(decl_idx).copied().unwrap_or("");
    let of = word_at(decl, decl_col + 1).unwrap_or_else(|| "?".to_string());
    if is_trait_decl(decl) {
        let raw_supertraits = supertraits(&header_from(&lines, decl_idx));
        let mut seen = HashSet::new();
        seen.insert(format!("{}:{decl_line}:{decl_col}", decl_file.display()));
        let mut list = Vec::with_capacity(raw_supertraits.len());
        for name in raw_supertraits {
            let mut node = Supertype::new(name.clone(), false, None);
            if depth > 1 {
                let bare_sub = name.split('<').next().unwrap_or(&name).trim();
                let mut sub_line = decl_line + 1;
                let mut sub_col = decl_col + 1;
                for (idx, line_str) in lines.iter().enumerate().skip(decl_idx) {
                    if let Some(pos) = line_str.find(bare_sub) {
                        sub_line = idx as u32 + 1;
                        sub_col = pos as u32 + 1;
                        break;
                    }
                    if line_str.contains(['{', ';']) {
                        break;
                    }
                }
                node.children = expand_rust_trait_supertraits(
                    remote,
                    root,
                    file,
                    &name,
                    &decl_file,
                    sub_line,
                    sub_col,
                    1,
                    depth,
                    &mut seen,
                )
                .await;
            }
            list.push(node);
        }
        return Ok(Supertypes {
            of,
            kind: Kind::Trait,
            list,
            depth,
            unsupported: None,
        });
    }
    let decl_uri = url::Url::from_file_path(&decl_file)
        .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {decl_file:?}"))?
        .to_string();
    let impls = execute_lsp_query(
        remote,
        root,
        &decl_file,
        "textDocument/implementation",
        serde_json::json!({
            "textDocument": { "uri": decl_uri },
            "position": { "line": decl_line, "character": decl_col },
        }),
    )
    .await?;
    let mut list: Vec<Supertype> = derives_above(&lines, decl_idx)
        .into_iter()
        .map(|(name, l, c)| Supertype::new(name, true, Some((decl_file.clone(), l, c))))
        .collect();
    for (path, l, c) in locations(&impls) {
        let text = if let Ok((bytes, _)) =
            crate::remote_fs::read_source(remote, root, &path.to_string_lossy()).await
        {
            String::from_utf8_lossy(&bytes).into_owned()
        } else {
            std::fs::read_to_string(&path).unwrap_or_default()
        };
        let lines: Vec<&str> = text.lines().collect();
        if l as usize >= lines.len() {
            continue;
        }
        // A derive, reported at its attribute or at the type's name, is already listed.
        let Some(start) = impl_header_start(&lines, l as usize) else {
            continue;
        };
        if let Some(name) = impl_trait(&header_from(&lines, start))
            && !list.iter().any(|s| s.name == name)
        {
            list.push(Supertype::new(name, false, Some((path.clone(), l + 1, c + 1))));
        }
    }
    list.sort_by(|a, b| a.name.cmp(&b.name));
    if depth > 1 {
        let mut seen = HashSet::new();
        seen.insert(format!("{}:{decl_line}:{decl_col}", decl_file.display()));
        for s in &mut list {
            if let Some((path, l, c)) = &s.at {
                s.children = expand_rust_trait_supertraits(
                    remote,
                    root,
                    file,
                    &s.name,
                    path,
                    *l,
                    *c,
                    1,
                    depth,
                    &mut seen,
                )
                .await;
            }
        }
    }
    Ok(Supertypes {
        of,
        kind: Kind::Type,
        list,
        depth,
        unsupported: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_impl_header_names_its_trait_and_an_inherent_one_none() {
        assert_eq!(
            impl_trait("impl Default for SearchIndexes ").as_deref(),
            Some("Default")
        );
        assert_eq!(
            impl_trait("impl<T: Clone> Into<Vec<T>> for Wrapper<T> where T: Send").as_deref(),
            Some("Into<Vec<T>>")
        );
        assert_eq!(
            impl_trait("unsafe impl Send for Handle").as_deref(),
            Some("Send")
        );
        assert_eq!(impl_trait("impl SearchIndexes "), None);
        assert_eq!(impl_trait("impl<T> Wrapper<T> where T: Fn() -> u8"), None);
        assert_eq!(impl_trait("fn main() {}"), None);
    }

    #[test]
    fn a_trait_header_names_its_supertraits() {
        assert_eq!(supertraits("pub trait Embed: Send "), vec!["Send"]);
        assert_eq!(
            supertraits(
                "trait Store<K: Ord>: Clone + Iterator<Item = (K, u8)> + 'static where K: Send"
            ),
            vec!["Clone", "Iterator<Item = (K, u8)>", "'static"]
        );
        assert!(supertraits("pub trait Plain ").is_empty());
        assert_eq!(
            supertraits("pub trait Circle where Self: Shape "),
            vec!["Shape"]
        );
        assert_eq!(
            supertraits("trait Both: Clone where Self: Shape + Clone, T: Copy, Self: Debug"),
            vec!["Clone", "Shape", "Debug"]
        );
        assert!(supertraits("trait Other where T: Copy ").is_empty());
        assert_eq!(supertraits("trait Near: Somewhere "), vec!["Somewhere"]);
        assert!(supertraits("struct Nope ").is_empty());
        assert!(is_trait_decl("pub(crate) unsafe trait Raw {"));
        assert!(!is_trait_decl("pub struct Traits;"));
    }

    #[test]
    fn a_header_is_joined_up_to_its_brace_and_a_word_is_read_at_a_column() {
        let lines = [
            "impl<T>",
            "    Default for Holder<T>",
            "where T: Default {",
            "}",
        ];
        assert_eq!(
            header_from(&lines, 0),
            "impl<T>     Default for Holder<T> where T: Default "
        );
        assert_eq!(
            word_at("#[derive(Clone, Debug)]", 10).as_deref(),
            Some("Clone")
        );
        assert_eq!(
            word_at("#[derive(Clone, Debug)]", 18).as_deref(),
            Some("Debug")
        );
        assert_eq!(word_at("#[derive(Clone)]", 1), None);
        assert_eq!(word_at("x", 0), None);
        assert_eq!(skip_generics("<A<B>> rest"), " rest");
        assert_eq!(skip_generics("<unclosed"), "");
    }

    #[test]
    fn derives_are_read_from_the_attributes_above_the_declaration() {
        let lines = [
            "}",
            "",
            "/// A report.",
            "#[derive(Debug, Clone,",
            "    serde::Serialize)]",
            "#[serde(tag = \"event\")]",
            "pub enum RunEvent {",
        ];
        assert_eq!(
            derives_above(&lines, 6),
            vec![
                ("Debug".to_string(), 4, 10),
                ("Clone".to_string(), 4, 17),
                ("serde::Serialize".to_string(), 5, 5),
            ]
        );
        assert!(derives_above(&["pub struct Bare;"], 0).is_empty());
        let impls = [
            "impl<T>",
            "    Default",
            "    for Holder<T> {",
            "}",
            "pub enum E {",
        ];
        assert_eq!(impl_header_start(&impls, 2), Some(0));
        assert_eq!(impl_header_start(&impls, 4), None);
    }

    #[test]
    fn the_report_says_what_there_is_and_what_there_is_not() {
        let root = Path::new("/w");
        let st = Supertypes {
            of: "Cache".into(),
            kind: Kind::Type,
            list: vec![
                Supertype::new("Clone", true, Some((PathBuf::from("/w/src/lib.rs"), 1, 10))),
                Supertype::new("Default", false, Some((PathBuf::from("/w/src/lib.rs"), 5, 18))),
            ],
            depth: 1,
            unsupported: None,
        };
        assert_eq!(
            st.render(root),
            "`Cache` implements 2 trait(s):\n  • Clone  (derived)  src/lib.rs:1:10\n  • Default  src/lib.rs:5:18"
        );
        let none = Supertypes {
            of: "Embed".into(),
            kind: Kind::Trait,
            list: vec![],
            depth: 1,
            unsupported: None,
        };
        assert_eq!(none.render(root), "`Embed` requires no supertrait.");
        let other = Supertypes {
            kind: Kind::Other,
            ..none.clone()
        };
        assert_eq!(other.render(root), "`Embed` has no supertype.");
        let no_server = Supertypes {
            unsupported: Some("No type hierarchy".into()),
            ..none
        };
        assert_eq!(no_server.render(root), "No type hierarchy");
        let link = serde_json::json!({ "targetUri": "file:///w/a.rs",
            "targetSelectionRange": { "start": { "line": 2, "character": 4 } } });
        assert_eq!(locations(&link), vec![(PathBuf::from("/w/a.rs"), 2, 4)]);
        assert!(locations(&serde_json::Value::Null).is_empty());
    }

    #[test]
    fn derives_above_never_panics_on_empty_lines_or_out_of_bounds_decl() {
        assert!(derives_above(&[], 0).is_empty());
        assert!(derives_above(&[], 232).is_empty());
        let lines = ["struct Foo;"];
        assert!(derives_above(&lines, 50).is_empty());
    }

    #[test]
    fn multi_level_supertypes_render_as_nested_tree() {
        let root = Path::new("/w");
        let mut parent = Supertype::new("Store", false, Some((PathBuf::from("/w/src/lib.rs"), 10, 5)));
        let child = Supertype::new("Send", false, None);
        let mut repeated_child = Supertype::new("Store", false, Some((PathBuf::from("/w/src/lib.rs"), 10, 5)));
        repeated_child.repeated = true;
        parent.children.push(child);
        parent.children.push(repeated_child);

        let st = Supertypes {
            of: "Cache".into(),
            kind: Kind::Type,
            list: vec![parent],
            depth: 2,
            unsupported: None,
        };
        assert_eq!(
            st.render(root),
            "`Cache` implements 1 trait(s), 3 in all to depth 2:\n  • Store  src/lib.rs:10:5\n    • Send\n    • Store  src/lib.rs:10:5  (shown above)"
        );
    }
}
