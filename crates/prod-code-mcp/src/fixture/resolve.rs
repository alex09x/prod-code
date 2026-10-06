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
use std::path::Path;

use anyhow::{Context, Result};

use crate::parameter_object::Language;
use crate::tools::{SymbolHit, workspace_symbol_search};

use super::polyglot::{PolyglotShape, parse_polyglot_shape};
use super::types::{Shape, split_top_level, strip_visibility};

/// The declaration body hover returns, without the markdown fences and the module line.
#[allow(dead_code)]
pub(crate) fn declaration_from_hover(hover: &str) -> Option<String> {
    let mut blocks = Vec::new();
    let mut current: Option<String> = None;
    for line in hover.lines() {
        if line.trim_start().starts_with("```") {
            match current.take() {
                Some(block) => blocks.push(block),
                None => current = Some(String::new()),
            }
            continue;
        }
        if let Some(block) = current.as_mut() {
            block.push_str(line);
            block.push('\n');
        }
    }
    // The first block is the module path, the one that declares the item is what we want.
    blocks.into_iter().find(|b| {
        let t = b.trim_start();
        t.starts_with("pub struct")
            || t.starts_with("struct")
            || t.starts_with("pub enum")
            || t.starts_with("enum")
    })
}

/// Reads a declaration into a shape. Doc comments and attributes are ignored.
pub fn parse_shape(decl: &str) -> Option<Shape> {
    let head = decl.trim_start();
    let is_enum = head.starts_with("enum") || head.starts_with("pub enum");
    let Some(open) = decl.find('{') else {
        // `struct Name(A, B);` or `struct Name;`
        if let Some(open) = decl.find('(') {
            let close = decl.rfind(')')?;
            let types = split_top_level(&decl[open + 1..close])
                .into_iter()
                .map(|t| strip_visibility(&t).to_string())
                .filter(|t| !t.is_empty())
                .collect();
            return Some(Shape::Tuple(types));
        }
        return Some(Shape::Unit);
    };
    let close = decl.rfind('}')?;
    let body = &decl[open + 1..close];
    if is_enum {
        let variants = body
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with("//") && !l.starts_with('#'))
            .filter_map(|l| {
                let name: String = l
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                (!name.is_empty()).then_some(name)
            })
            .collect();
        return Some(Shape::Enum(variants));
    }
    let mut fields = Vec::new();
    for raw in split_top_level(body) {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("//") || line.starts_with('#') {
            continue;
        }
        let line = strip_visibility(line);
        let Some((name, ty)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
            continue;
        }
        fields.push((name.to_string(), ty.trim().to_string()));
    }
    Some(Shape::Record(fields))
}

/// The source text of the declaration that `line` (1-based) belongs to, taken from the
/// document symbols and the file itself so nothing is elided.
pub(crate) fn declaration_at(symbols: &serde_json::Value, line: u32, text: &str) -> Option<String> {
    fn walk(nodes: &[serde_json::Value], line: u32, best: &mut Option<(u32, u32)>) {
        for node in nodes {
            let range = node
                .get("range")
                .or_else(|| node.get("location").and_then(|l| l.get("range")));
            if let Some(range) = range
                && let (Some(start), Some(end)) = (range.get("start"), range.get("end"))
                && let (Some(s), Some(e)) = (
                    start.get("line").and_then(|l| l.as_u64()),
                    end.get("line").and_then(|l| l.as_u64()),
                )
            {
                let (s, e) = (s as u32 + 1, e as u32 + 1);
                if s <= line && line <= e && best.is_none_or(|(bs, be)| e - s < be - bs) {
                    *best = Some((s, e));
                }
            }
            if let Some(children) = node.get("children").and_then(|c| c.as_array()) {
                walk(children, line, best);
            }
        }
    }
    let mut best = None;
    walk(symbols.as_array().map(|a| a.as_slice())?, line, &mut best);
    let (start, end) = best?;
    Some(
        text.lines()
            .skip(start.saturating_sub(1) as usize)
            .take((end.saturating_sub(start) + 1) as usize)
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// The declaration of a *type* called `name`.
pub(crate) async fn resolve_type(
    remote: SocketAddr,
    root: &Path,
    name: &str,
    hint: Option<&Path>,
) -> Result<SymbolHit> {
    let hits = workspace_symbol_search(remote, root, name, hint, 32).await?;
    let mut types: Vec<SymbolHit> = hits
        .into_iter()
        .filter(|h| {
            h.name == name
                && matches!(
                    h.kind,
                    "Struct" | "Enum" | "Class" | "Interface" | "TypeParameter" | "Object"
                )
        })
        .collect();
    if types.is_empty() {
        let any_hits = workspace_symbol_search(remote, root, name, hint, 32).await?;
        types = any_hits.into_iter().filter(|h| h.name == name).collect();
    }
    if let Some(hint) = hint {
        let in_hint: Vec<SymbolHit> = types.iter().filter(|h| h.path == hint).cloned().collect();
        if !in_hint.is_empty() {
            types = in_hint;
        }
    }
    types.sort_by(|a, b| (&a.path, a.line, a.col).cmp(&(&b.path, b.line, b.col)));
    types.dedup_by(|a, b| a.path == b.path && a.line == b.line && a.col == b.col);
    match types.len() {
        0 => anyhow::bail!("no struct or enum named `{name}` in this workspace"),
        1 => Ok(types.remove(0)),
        _ => {
            let list = types
                .iter()
                .map(|h| {
                    format!(
                        "  [{}] {} — {}:{}:{}",
                        h.kind,
                        h.name,
                        h.path
                            .strip_prefix(root)
                            .unwrap_or(&h.path)
                            .to_string_lossy(),
                        h.line,
                        h.col
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            anyhow::bail!(
                "`{name}` is declared in more than one file or position; pass `path` to the declaring file (multiple declarations in one file still need a unique name):\n{list}"
            );
        }
    }
}

pub(crate) fn extract_decl_around_line(text: &str, line: u32) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    if line == 0 || line as usize > lines.len() {
        return None;
    }
    let idx = (line - 1) as usize;
    let mut start = idx;
    while start > 0
        && !lines[start].contains('{')
        && !lines[start].trim_start().starts_with("class ")
        && !lines[start].trim_start().starts_with("type ")
        && !lines[start].trim_start().starts_with("struct ")
        && !lines[start].trim_start().starts_with("interface ")
        && !lines[start].trim_start().starts_with("pub ")
    {
        start -= 1;
    }
    let mut end = idx;
    let mut brace_depth = 0i32;
    let mut found_brace = false;
    for (i, line) in lines.iter().enumerate().skip(start) {
        for c in line.chars() {
            if c == '{' {
                brace_depth += 1;
                found_brace = true;
            } else if c == '}' {
                brace_depth -= 1;
            }
        }
        end = i;
        if found_brace && brace_depth <= 0 {
            break;
        }
    }
    Some(lines[start..=end].join("\n"))
}

/// The polyglot shape of a named type and the file that declares it.
pub async fn shape_of_polyglot(
    remote: SocketAddr,
    root: &Path,
    name: &str,
    hint: Option<&Path>,
    explicit_lang: Option<Language>,
) -> Result<(PolyglotShape, String, Language)> {
    let hit = resolve_type(remote, root, name, hint).await?;
    let lang = explicit_lang
        .or_else(|| Language::of(&hit.path))
        .unwrap_or(Language::Rust);
    let uri = url::Url::from_file_path(&hit.path)
        .map_err(|_| anyhow::anyhow!("invalid path {:?}", hit.path))?
        .to_string();
    let symbols = crate::tools::execute_lsp_query(
        remote,
        root,
        &hit.path,
        "textDocument/documentSymbol",
        serde_json::json!({ "textDocument": { "uri": uri } }),
    )
    .await
    .unwrap_or(serde_json::Value::Null);
    let text = std::fs::read_to_string(&hit.path)
        .with_context(|| format!("cannot read {}", hit.path.display()))?;
    let decl = declaration_at(&symbols, hit.line, &text)
        .or_else(|| extract_decl_around_line(&text, hit.line))
        .with_context(|| {
            format!(
                "no declaration of `{name}` at {}:{}",
                hit.path.display(),
                hit.line
            )
        })?;
    let poly_shape = parse_polyglot_shape(&decl, lang)
        .or_else(|| {
            parse_shape(&decl).map(|s| match s {
                Shape::Record(f) => PolyglotShape::Record(f),
                Shape::Tuple(t) => PolyglotShape::Tuple(t),
                Shape::Unit => PolyglotShape::Unit,
                Shape::Enum(v) => PolyglotShape::Enum(v),
            })
        })
        .with_context(|| format!("cannot read the shape of `{name}`"))?;
    let file = hit
        .path
        .strip_prefix(root)
        .unwrap_or(&hit.path)
        .to_string_lossy()
        .into_owned();
    Ok((poly_shape, file, lang))
}

/// The shape of a named type and the file that declares it.
pub async fn shape_of(
    remote: SocketAddr,
    root: &Path,
    name: &str,
    hint: Option<&Path>,
) -> Result<(Shape, String)> {
    let (poly, file, _lang) = shape_of_polyglot(remote, root, name, hint, None).await?;
    let shape = match poly {
        PolyglotShape::Record(f) => Shape::Record(f),
        PolyglotShape::Tuple(t) => Shape::Tuple(t),
        PolyglotShape::Unit => Shape::Unit,
        PolyglotShape::Enum(v) => Shape::Enum(v),
        PolyglotShape::Interface { .. } => Shape::Unit,
        PolyglotShape::InterfaceWithFields { .. } => Shape::Unit,
    };
    Ok((shape, file))
}
