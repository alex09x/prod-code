/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Syntax and original byte ranges for Rust moves. Syntax never supplies crate identity.

use anyhow::{Context, Result};
use ra_ap_syntax::{AstNode, Edition, SourceFile, SyntaxNode, ast, ast::HasName};
use std::{ops::Range, path::Path};

mod configuration;
mod paths;
pub(crate) use configuration::{Condition, active_path_literals, scope_condition};
pub(crate) use paths::rebase_super;
pub(super) use paths::{grouped_self, in_visibility};

pub(crate) fn edition(file: &Path) -> Edition {
    for dir in file.ancestors().skip(1) {
        let Ok(text) = std::fs::read_to_string(dir.join("Cargo.toml")) else {
            continue;
        };
        let Ok(manifest) = toml::from_str::<toml::Value>(&text) else {
            continue;
        };
        let Some(package) = manifest.get("package") else {
            continue;
        };
        if let Some(value) = package.get("edition").and_then(|e| e.as_str()) {
            return value.parse().unwrap_or(Edition::Edition2015);
        }
        if package
            .get("edition")
            .and_then(|e| e.get("workspace"))
            .and_then(|w| w.as_bool())
            == Some(true)
        {
            for ancestor in dir.ancestors() {
                let Ok(text) = std::fs::read_to_string(ancestor.join("Cargo.toml")) else {
                    continue;
                };
                let Ok(doc) = toml::from_str::<toml::Value>(&text) else {
                    continue;
                };
                if let Some(value) = doc
                    .get("workspace")
                    .and_then(|w| w.get("package"))
                    .and_then(|p| p.get("edition"))
                    .and_then(|e| e.as_str())
                {
                    return value.parse().unwrap_or(Edition::Edition2015);
                }
            }
        }
        return Edition::Edition2015;
    }
    Edition::Edition2015
}

pub(crate) fn parse(text: &str, edition: Edition) -> SourceFile {
    SourceFile::parse(text, edition).tree()
}

pub(crate) fn impl_header_for_owner(
    text: &str,
    (impl_start, impl_open): (usize, usize),
    owner: &str,
    qualified_owner: &str,
    edition: Edition,
) -> Option<String> {
    let source = parse(text, edition);
    let implementation = source
        .syntax()
        .descendants()
        .filter_map(ast::Impl::cast)
        .find(|implementation| range(implementation.syntax()).start == impl_start)?;
    let self_type = implementation.self_ty()?;
    let path_type = ast::PathType::cast(self_type.syntax().clone())?;
    let path = path_type.path()?;
    let owner_name = path.segments().last()?.name_ref()?;
    if owner_name.text() != owner {
        return None;
    }
    let path_start = range(path.syntax()).start.checked_sub(impl_start)?;
    let owner_end = range(owner_name.syntax()).end.checked_sub(impl_start)?;
    let mut header = text.get(impl_start..=impl_open)?.to_string();
    header.replace_range(path_start..owner_end, qualified_owner);
    Some(header)
}

pub(crate) fn range(node: &SyntaxNode) -> Range<usize> {
    let r = node.text_range();
    usize::from(r.start())..usize::from(r.end())
}

pub(crate) fn item_at(text: &str, line: u32, edition: Edition) -> Option<SyntaxNode> {
    let start = crate::signature::offset_of(text, line, 1)?;
    let end = text[start..].find('\n').map_or(text.len(), |n| start + n);
    parse(text, edition)
        .syntax()
        .descendants()
        .filter(|n| ast::Item::can_cast(n.kind()) && range(n).start <= end && range(n).end > start)
        // Native symbols start at the declaration name, possibly after split visibility.
        .filter(|n| {
            n.children().find_map(ast::Name::cast).is_some_and(|name| {
                let r = range(name.syntax());
                r.start >= start && r.start <= end
            })
        })
        .min_by_key(|n| range(n).len())
}

pub(crate) fn item_start(text: &str, line: u32, edition: Edition) -> u32 {
    item_at(text, line, edition).map_or(line, |node| {
        text[..range(&node).start]
            .bytes()
            .filter(|b| *b == b'\n')
            .count() as u32
            + 1
    })
}

pub(crate) fn public(text: &str, line: u32, edition: Edition) -> bool {
    item_at(text, line, edition)
        .is_some_and(|node| node.children().find_map(ast::Visibility::cast).is_some())
}

/// The current planner cuts whole lines so reference coordinates below the cut remain exact.
/// Refuse a cut that would also remove another same-line declaration, even when forced.
pub(crate) fn ensure_whole_lines(
    text: &str,
    line: u32,
    name: &str,
    edition: Edition,
) -> Result<()> {
    let node =
        item_at(text, line, edition).context("selected declaration has no Rust syntax node")?;
    anyhow::ensure!(
        node.children()
            .find_map(ast::Name::cast)
            .is_some_and(|n| n.text() == name),
        "selected syntax and analyzer declaration differ; nothing was written"
    );
    let span = range(&node);
    let prefix = &text[line_start(text, span.start)..span.start];
    let tail = &text[span.end
        ..text[span.end..]
            .find('\n')
            .map_or(text.len(), |n| span.end + n)];
    anyhow::ensure!(
        prefix.trim().is_empty() && (tail.trim().is_empty() || tail.trim_start().starts_with("//")),
        "whole-line item move would remove neighboring syntax; nothing was written"
    );
    Ok(())
}

#[derive(Clone, Debug)]
pub(crate) struct Declaration {
    pub name: String,
    pub span: Range<usize>,
    pub name_at: usize,
    pub path: Option<String>,
    pub cfg_paths: Vec<String>,
    pub unresolvable_path: bool,
    pub condition: Condition,
    pub inline: Option<Range<usize>>,
}

/// Only immediate module children of this lexical scope, never a nested same-name match.
pub(crate) fn declarations(text: &str, scope: Range<usize>, edition: Edition) -> Vec<Declaration> {
    let tree = parse(text, edition);
    let container = tree.syntax().descendants().find(|n| {
        range(n) == scope
            && (ast::SourceFile::can_cast(n.kind()) || ast::ItemList::can_cast(n.kind()))
    });
    let Some(container) = container else {
        return Vec::new();
    };
    container
        .children()
        .filter_map(ast::Module::cast)
        .filter_map(|module| {
            let name = module.name()?;
            let metadata = configuration::module(&module);
            Some(Declaration {
                name: name.text().to_string(),
                span: range(module.syntax()),
                name_at: range(name.syntax()).start,
                path: metadata.path,
                cfg_paths: metadata.alternatives,
                unresolvable_path: metadata.unresolved,
                condition: metadata.condition,
                inline: module.item_list().map(|items| range(items.syntax())),
            })
        })
        .collect()
}

pub(crate) fn declaration(text: &str, name: &str) -> Option<(usize, usize, String)> {
    declaration_in(text, name, Edition::CURRENT)
}

pub(crate) fn declaration_in(
    text: &str,
    name: &str,
    edition: Edition,
) -> Option<(usize, usize, String)> {
    let decl = declarations(text, 0..text.len(), edition)
        .into_iter()
        .find(|d| d.name == name && d.inline.is_none())?;
    let end = line_end(text, decl.span.end);
    let start = line_start(text, decl.span.start);
    let start = if text[start..decl.span.start].trim().is_empty() {
        start
    } else {
        decl.span.start
    };
    Some((
        start,
        end,
        text[line_start(text, decl.name_at)..end]
            .trim_end()
            .to_string(),
    ))
}

pub(crate) fn line_start(text: &str, at: usize) -> usize {
    text[..at].rfind('\n').map_or(0, |n| n + 1)
}

pub(crate) fn line_end(text: &str, at: usize) -> usize {
    // Do not consume a following declaration on the same physical line.
    let tail = &text[at..];
    let end = tail.find('\n').map_or(text.len(), |n| at + n + 1);
    if text[at..end].trim().is_empty() {
        end
    } else {
        at
    }
}

pub(crate) fn read(file: &Path) -> Result<String> {
    std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))
}
