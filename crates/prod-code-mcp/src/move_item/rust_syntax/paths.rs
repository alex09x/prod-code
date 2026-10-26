/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{parse, range};
use anyhow::Result;
use ra_ap_syntax::{AstNode, Edition, ast};

/// Restrictions live below VISIBILITY_INNER in the pinned parser, not directly below
/// VISIBILITY. They have their own Rust grammar and must never be rebased as value paths.
pub(crate) fn in_visibility(path: &ast::Path) -> bool {
    path.syntax()
        .ancestors()
        .any(|n| ast::Visibility::can_cast(n.kind()))
}

/// A grouped `self` leaf refers to its use-tree prefix, including `self as alias`.
/// It is not the current module, unlike the head of `self::item`.
pub(crate) fn grouped_self(path: &ast::Path) -> bool {
    path.syntax()
        .parent()
        .and_then(ast::UseTree::cast)
        .is_some_and(|tree| {
            tree.syntax()
                .parent()
                .is_some_and(|n| ast::UseTreeList::can_cast(n.kind()))
        })
}

/// Rebase only lexical paths escaping the moved subtree. Token trees, literals, comments
/// and paths to nodes still inside the subtree retain their original bytes and scope.
pub(crate) fn rebase_super(
    text: &str,
    old: &[String],
    depth: usize,
    edition: Edition,
) -> Result<(String, usize)> {
    let tree = parse(text, edition);
    let mut edits = Vec::new();
    let mut count = 0;
    for path in tree.syntax().descendants().filter_map(ast::Path::cast) {
        if path.qualifier().is_some() {
            continue;
        }
        let Some(segment) = path.segment() else {
            continue;
        };
        if segment.syntax().text().to_string() != "super" {
            continue;
        }
        // `pub(super)` is a visibility restriction, not a `super::` path head.
        if in_visibility(&path) {
            continue;
        }
        let nested = path
            .syntax()
            .ancestors()
            .filter_map(ast::Module::cast)
            .filter(|m| m.item_list().is_some())
            .count()
            + depth;
        let mut chain = path.clone();
        let mut tails = Vec::new();
        let mut supers = 1;
        while let Some(parent) = chain.syntax().parent().and_then(ast::Path::cast) {
            if let Some(segment) = parent
                .segment()
                .filter(|s| s.syntax().text().to_string() == "super")
            {
                tails.push(range(segment.syntax()));
                if let Some(sep) = parent
                    .syntax()
                    .children_with_tokens()
                    .filter_map(|n| n.into_token())
                    .find(|t| t.text() == "::")
                {
                    let r = sep.text_range();
                    tails.push(usize::from(r.start())..usize::from(r.end()));
                }
                chain = parent;
                supers += 1;
            } else {
                break;
            }
        }
        if supers <= nested {
            continue;
        }
        let up = supers - nested;
        anyhow::ensure!(
            up <= old.len(),
            "super path escapes its known Cargo root; nothing was written"
        );
        let mut parts = vec!["crate".to_string()];
        parts.extend_from_slice(&old[..old.len() - up]);
        edits.push((range(segment.syntax()), parts.join("::")));
        // Remove syntax tokens only, keeping inter-segment comments and whitespace intact.
        edits.extend(tails.into_iter().map(|r| (r, String::new())));
        count += 1;
    }
    edits.sort_by_key(|(r, _)| r.start);
    let mut out = text.to_string();
    for (span, replacement) in edits.into_iter().rev() {
        out.replace_range(span, &replacement);
    }
    Ok((out, count))
}
