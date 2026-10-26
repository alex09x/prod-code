/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Remove only bindings of the arriving module in its exact destination scope.

use crate::move_item::rust_syntax::{parse, range};
use anyhow::{Context, Result};
use ra_ap_syntax::{AstNode, Edition, ast, ast::HasName};
use std::ops::Range;

/// A reference inside a use tree carries its suffix and alias with it. Detach a grouped
/// branch into its own use at the same lexical scope instead of dropping its binding.
pub(super) fn rewrite_reference(
    text: &str,
    offset: usize,
    new_path: &str,
    edition: Edition,
) -> Option<String> {
    let source = parse(text, edition);
    let segment = source
        .syntax()
        .descendants()
        .filter_map(ast::PathSegment::cast)
        .find(|s| range(s.syntax()).start == offset)?;
    let tree = segment.syntax().ancestors().find_map(ast::UseTree::cast)?;
    let import = tree.syntax().ancestors().find_map(ast::Use::cast)?;
    let path = tree.path()?;
    let start = range(path.syntax()).start;
    let end = range(segment.syntax()).end;
    if !tree
        .syntax()
        .parent()
        .is_some_and(|n| ast::UseTreeList::can_cast(n.kind()))
    {
        let mut out = text.to_string();
        out.replace_range(start..end, new_path);
        return Some(out);
    }
    let span = range(tree.syntax());
    let mut branch = text[span.clone()].to_string();
    branch.replace_range(start - span.start..end - span.start, new_path);
    let root_tree = import.use_tree()?;
    let import_span = range(import.syntax());
    let prefix = &text[import_span.start..range(root_tree.syntax()).start];
    let statement = format!("\n{prefix}{branch};");
    // Collapse empty ancestor groups when this was their only branch.
    let mut remove = tree;
    loop {
        let list = remove.syntax().parent().and_then(ast::UseTreeList::cast)?;
        if list.use_trees().count() > 1 {
            let removed = range(remove.syntax());
            let comma = list
                .syntax()
                .children_with_tokens()
                .filter_map(|n| n.into_token())
                .filter(|t| t.text() == ",")
                .map(|t| usize::from(t.text_range().start())..usize::from(t.text_range().end()))
                .collect::<Vec<_>>();
            let comma = comma
                .iter()
                .find(|r| r.start >= removed.end)
                .or_else(|| comma.iter().rev().find(|r| r.end <= removed.start))?;
            let mut out = text.to_string();
            out.insert_str(import_span.end, &statement);
            let mut spans = [removed, comma.clone()];
            spans.sort_by_key(|r| r.start);
            for span in spans.into_iter().rev() {
                out.replace_range(span, "");
            }
            return Some(out);
        }
        remove = list.syntax().parent().and_then(ast::UseTree::cast)?;
        if remove == root_tree {
            let mut out = text.to_string();
            out.replace_range(import_span, statement.trim_start_matches('\n'));
            return Some(out);
        }
    }
}

pub(super) fn remove_arriving_imports(
    text: &str,
    scope: Range<usize>,
    parent: &[String],
    old: &[String],
    new: &[String],
    edition: Edition,
) -> Result<String> {
    let source = parse(text, edition);
    let container = source
        .syntax()
        .descendants()
        .find(|n| {
            range(n) == scope
                && (ast::SourceFile::can_cast(n.kind()) || ast::ItemList::can_cast(n.kind()))
        })
        .context("destination import scope changed; nothing was written")?;
    let mut edits = Vec::new();
    for import in container.children().filter_map(ast::Use::cast) {
        let Some(tree) = import.use_tree() else {
            continue;
        };
        if prune(&tree, &[], parent, old, new, &mut edits)? {
            edits.push(range(import.syntax()));
        }
    }
    edits.sort_by_key(|r| (r.start, std::cmp::Reverse(r.end)));
    let mut spans: Vec<Range<usize>> = Vec::new();
    for span in edits {
        if spans.last().is_none_or(|r| r.end <= span.start) {
            spans.push(span);
        }
    }
    let mut out = text.to_string();
    for span in spans.into_iter().rev() {
        out.replace_range(span, "");
    }
    Ok(out)
}

fn prune(
    tree: &ast::UseTree,
    prefix: &[String],
    parent: &[String],
    old: &[String],
    new: &[String],
    edits: &mut Vec<Range<usize>>,
) -> Result<bool> {
    let mut path = prefix.to_vec();
    if let Some(p) = tree.path() {
        path.extend(
            p.syntax()
                .descendants()
                .filter_map(ast::PathSegment::cast)
                .map(|s| s.syntax().text().to_string()),
        );
    }
    if let Some(list) = tree.use_tree_list() {
        let children: Vec<_> = list.use_trees().collect();
        let mut removed = Vec::new();
        for child in &children {
            removed.push(prune(child, &path, parent, old, new, edits)?);
        }
        if removed.iter().all(|v| *v) {
            return Ok(true);
        }
        let commas: Vec<_> = list
            .syntax()
            .children_with_tokens()
            .filter_map(|n| n.into_token())
            .filter(|t| t.text() == ",")
            .map(|t| usize::from(t.text_range().start())..usize::from(t.text_range().end()))
            .collect();
        // Each removed leaf consumes its following comma, or the previous comma when last.
        // Deduplication above handles adjacent removed leaves without touching other names.
        for (_, child) in children.iter().enumerate().filter(|(i, _)| removed[*i]) {
            let span = range(child.syntax());
            if let Some(comma) = commas
                .iter()
                .find(|r| r.start >= span.end)
                .or_else(|| commas.iter().rev().find(|r| r.end <= span.start))
            {
                edits.push(comma.clone());
            }
            edits.push(span);
        }
        return Ok(false);
    }
    if tree.star_token().is_some() {
        return Ok(false);
    }
    let binding = tree
        .rename()
        .map(|r| {
            r.name()
                .map_or_else(|| "_".to_string(), |n| n.text().to_string())
        })
        .or_else(|| path.iter().rev().find(|p| p.as_str() != "self").cloned());
    if binding.as_ref() != new.last() {
        return Ok(false);
    }
    // A trailing grouped self imports the prefix itself, rather than a module named self.
    if path.last().is_some_and(|p| p == "self") {
        path.pop();
    }
    let resolved = match path.first().map(String::as_str) {
        Some("crate") => path[1..].to_vec(),
        Some("self") => parent.iter().chain(&path[1..]).cloned().collect(),
        Some("super") => {
            let supers = path.iter().take_while(|p| p.as_str() == "super").count();
            anyhow::ensure!(
                supers <= parent.len(),
                "import escapes destination Cargo root; nothing was written"
            );
            parent[..parent.len() - supers]
                .iter()
                .chain(&path[supers..])
                .cloned()
                .collect()
        }
        _ => path,
    };
    anyhow::ensure!(
        resolved == old || resolved == new,
        "destination import binds the arriving module name to another path; nothing was written"
    );
    Ok(true)
}
