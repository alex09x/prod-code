/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::rust_crate::CrateRoot;
use super::rust_lookup::{resolve, resolve_names};
use ra_ap_syntax::{AstNode, SyntaxNode, ast};

pub(super) fn selected_declaration(
    root: &SyntaxNode,
    start: usize,
    end: usize,
) -> Option<SyntaxNode> {
    root.descendants()
        .filter_map(ast::Struct::cast)
        .find_map(|item| {
            let range = item.syntax().text_range();
            (usize::from(range.start()) <= start && usize::from(range.end()) == end)
                .then(|| item.syntax().clone())
        })
}

/// A lexical declaration's node is its identity; a repeated bare name is not.
pub(super) fn is_selected(
    path: &ast::Path,
    selected: Option<&SyntaxNode>,
    name: &str,
    crate_root: &CrateRoot,
    referenced: impl Fn(&ast::Path) -> bool,
) -> Result<bool, ()> {
    let is_self = path
        .as_single_name_ref()
        .is_some_and(|n| n.text() == "Self");
    let candidate = if is_self {
        let Some(owner) = path.syntax().ancestors().find_map(ast::Impl::cast) else {
            return Ok(false);
        };
        let Some(ast::Type::PathType(ty)) = owner.self_ty() else {
            return Ok(false);
        };
        let Some(path) = ty.path() else {
            return Ok(false);
        };
        path
    } else {
        path.clone()
    };
    if referenced(&candidate) {
        return Ok(true);
    }
    let foreign_self = is_self
        && candidate
            .as_single_name_ref()
            .is_some_and(|owner| owner.text() != name);
    match (resolve(&candidate, crate_root)?, selected) {
        (Some(actual), Some(selected)) => Ok(&actual == selected),
        (Some(_), None) => Ok(false),
        // Preserve exclusion of bare foreign impl owners in source-only excerpts.
        (None, _) if foreign_self => Ok(false),
        // Missing qualified owners are unknown, not selected by final spelling.
        // The exact semantic reference check above can still prove selected sites.
        (None, _) => Err(()),
    }
}

/// Macro tokens retain their lexical context, but have no expression AST until
/// expansion. Disprove unrelated owners where possible; refuse unknown identity.
pub(super) fn opaque_is_selected(
    context: &SyntaxNode,
    names: &[String],
    selected: Option<&SyntaxNode>,
    type_name: &str,
    crate_root: &CrateRoot,
    referenced: impl Fn(&ast::Path) -> bool,
) -> bool {
    if names == ["Self"] {
        let Some(owner) = context.ancestors().find_map(ast::Impl::cast) else {
            return false;
        };
        let Some(ast::Type::PathType(ty)) = owner.self_ty() else {
            return false;
        };
        return ty.path().is_some_and(|path| {
            is_selected(&path, selected, type_name, crate_root, referenced).unwrap_or(true)
        });
    }
    match (resolve_names(context, names, crate_root), selected) {
        (Ok(Some(actual)), Some(selected)) => &actual == selected,
        (Ok(Some(_)), None) => false,
        (Ok(None) | Err(()), _) => true,
    }
}
