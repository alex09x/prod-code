/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::ops::Range;
use std::path::{Path, PathBuf};

use anyhow::Result;
use ra_ap_syntax::{
    AstNode, Edition, SourceFile, SyntaxKind, SyntaxNode, ast, ast::HasName, ast::HasVisibility,
};

use super::access::{
    PendingAccess, collect_dot_field_candidates, resolve_pending_accesses, same_file,
};
use super::macros::{check_macro_trees, inside_token_tree};
use super::{NeededFields, PrivateFields, canonical_ident};
use crate::move_item::budget::MoveBudget;
use crate::move_item::item::{display, offset_of};
use crate::move_item::rust_syntax;

#[allow(clippy::too_many_arguments)]
pub(super) async fn scan_scope_file(
    budget: &MoveBudget,
    remote: SocketAddr,
    root: &Path,
    source_file: &Path,
    scope_path: &Path,
    text: &str,
    scope_ranges: &[Range<usize>],
    excluded: Option<Range<usize>>,
    struct_name: &str,
    shape: &PrivateFields,
    struct_refs: &[(PathBuf, u32, u32)],
    edition: Edition,
    needed: &mut NeededFields,
) -> Result<()> {
    let pending = collect_file_syntax_usage(
        root,
        scope_path,
        text,
        scope_ranges,
        excluded,
        struct_name,
        shape,
        struct_refs,
        edition,
        needed,
    )?;
    if needed.is_complete(shape) || pending.is_empty() {
        return Ok(());
    }
    resolve_pending_accesses(
        budget,
        remote,
        root,
        source_file,
        scope_path,
        struct_name,
        shape,
        pending,
        needed,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
fn collect_file_syntax_usage(
    root: &Path,
    scope_path: &Path,
    text: &str,
    scope_ranges: &[Range<usize>],
    excluded: Option<Range<usize>>,
    struct_name: &str,
    shape: &PrivateFields,
    struct_refs: &[(PathBuf, u32, u32)],
    edition: Edition,
    needed: &mut NeededFields,
) -> Result<Vec<PendingAccess>> {
    let in_scope = |offset: usize| {
        scope_ranges.iter().any(|r| r.contains(&offset))
            && excluded.as_ref().is_none_or(|ex| !ex.contains(&offset))
    };

    let mut ref_coords = BTreeSet::new();
    for (ref_path, line, col) in struct_refs {
        if !same_file(ref_path, scope_path) {
            continue;
        }
        let Some(at) = offset_of(text, *line, *col) else {
            continue;
        };
        if !in_scope(at) {
            continue;
        }
        if text[at..].starts_with(struct_name)
            && !text[at + struct_name.len()..]
                .starts_with(|c: char| c.is_alphanumeric() || c == '_')
        {
            ref_coords.insert((*line, *col));
        }
    }

    let tree = rust_syntax::parse(text, edition);
    let impl_ranges = find_struct_impl_ranges(&tree, text, &in_scope, &ref_coords);
    let is_self_of_s = |node: &SyntaxNode| {
        node.ancestors()
            .find(|a| ast::Impl::can_cast(a.kind()) || ast::Trait::can_cast(a.kind()))
            .is_some_and(|enc| impl_ranges.contains(&rust_syntax::range(&enc)))
    };
    let path_is_moved = |path: &ast::Path| {
        if path
            .as_single_name_ref()
            .is_some_and(|nr| nr.text() == "Self")
        {
            return is_self_of_s(path.syntax());
        }
        path.segments()
            .last()
            .and_then(|s| s.name_ref())
            .and_then(|nr| {
                crate::signature::line_col_at(text, rust_syntax::range(nr.syntax()).start)
            })
            .is_some_and(|lc| ref_coords.contains(&lc))
    };

    check_type_aliases(
        root,
        scope_path,
        text,
        &tree,
        &in_scope,
        &ref_coords,
        struct_name,
    )?;
    check_macro_trees(
        root,
        scope_path,
        text,
        &tree,
        &in_scope,
        &ref_coords,
        &is_self_of_s,
        struct_name,
        edition,
        shape,
        needed,
    )?;

    for node in tree.syntax().descendants() {
        let start = rust_syntax::range(&node).start;
        if !in_scope(start) || inside_token_tree(&node) {
            continue;
        }
        if let Some(rec) = ast::RecordExpr::cast(node.clone()) {
            if rec.path().is_some_and(|p| path_is_moved(&p)) {
                needed.mark_all(shape);
            }
        } else if let Some(pat) = ast::RecordPat::cast(node.clone()) {
            if pat.path().is_some_and(|p| path_is_moved(&p))
                && let Some(list) = pat.record_pat_field_list()
            {
                for field in list.fields() {
                    if let Some(name) = field.field_name() {
                        let ident = canonical_ident(name.text());
                        if let PrivateFields::Record { unique_names, .. } = shape
                            && unique_names.contains(&ident)
                        {
                            needed.record_names.insert(ident);
                        }
                    }
                }
            }
        } else if let Some(pat) = ast::TupleStructPat::cast(node.clone()) {
            if pat.path().is_some_and(|p| path_is_moved(&p)) {
                needed.mark_all(shape);
            }
        } else if let Some(expr) = ast::PathExpr::cast(node)
            && matches!(shape, PrivateFields::Tuple { .. })
            && expr.path().is_some_and(|p| path_is_moved(&p))
        {
            needed.mark_all(shape);
        }
    }

    if needed.is_complete(shape) {
        return Ok(Vec::new());
    }
    collect_dot_field_candidates(text, &tree, &in_scope, &is_self_of_s, shape, needed)
}

fn find_struct_impl_ranges(
    tree: &SourceFile,
    text: &str,
    in_scope: &impl Fn(usize) -> bool,
    ref_coords: &BTreeSet<(u32, u32)>,
) -> Vec<Range<usize>> {
    tree.syntax()
        .descendants()
        .filter_map(ast::Impl::cast)
        .filter(|imp| in_scope(rust_syntax::range(imp.syntax()).start))
        .filter(|imp| {
            unwrap_path_type(imp.self_ty())
                .and_then(|pt| pt.path())
                .and_then(|p| p.segments().last())
                .and_then(|s| s.name_ref())
                .and_then(|nr| {
                    crate::signature::line_col_at(text, rust_syntax::range(nr.syntax()).start)
                })
                .is_some_and(|lc| ref_coords.contains(&lc))
        })
        .map(|imp| rust_syntax::range(imp.syntax()))
        .collect()
}

fn unwrap_path_type(mut ty: Option<ast::Type>) -> Option<ast::PathType> {
    while let Some(inner) = ty {
        match inner {
            ast::Type::PathType(pt) => return Some(pt),
            ast::Type::ParenType(paren) => ty = paren.ty(),
            _ => return None,
        }
    }
    None
}

fn check_type_aliases(
    root: &Path,
    scope_path: &Path,
    text: &str,
    tree: &SourceFile,
    in_scope: &impl Fn(usize) -> bool,
    ref_coords: &BTreeSet<(u32, u32)>,
    struct_name: &str,
) -> Result<()> {
    for alias in tree.syntax().descendants().filter_map(ast::TypeAlias::cast) {
        let start = rust_syntax::range(alias.syntax()).start;
        if !in_scope(start) {
            continue;
        }
        let aliases_struct = unwrap_path_type(alias.ty())
            .and_then(|pt| pt.path())
            .and_then(|p| p.segments().last())
            .and_then(|s| s.name_ref())
            .and_then(|nr| {
                crate::signature::line_col_at(text, rust_syntax::range(nr.syntax()).start)
            })
            .is_some_and(|lc| ref_coords.contains(&lc));
        if !aliases_struct {
            continue;
        }
        let Some(name_node) = alias.name() else {
            continue;
        };
        let alias_ident = canonical_ident(name_node.text());
        let (line, col) = crate::signature::position_at(text, start).unwrap_or((1, 1));
        if alias.visibility().is_some() || alias_used_beyond_types(tree, in_scope, &alias_ident) {
            anyhow::bail!(
                "cannot prove minimum field visibility for `{struct_name}` through type alias `{}` \
                 at {}:{line}:{col}; nothing was written",
                name_node.text(),
                display(root, scope_path)
            );
        }
    }
    Ok(())
}

fn alias_used_beyond_types(
    tree: &SourceFile,
    in_scope: &impl Fn(usize) -> bool,
    alias_ident: &str,
) -> bool {
    tree.syntax()
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| t.kind() == SyntaxKind::IDENT && canonical_ident(t.text()) == alias_ident)
        .filter(|t| in_scope(usize::from(t.text_range().start())))
        .any(|tok| {
            let Some(parent) = tok.parent() else {
                return false;
            };
            if ast::TokenTree::can_cast(parent.kind()) {
                return true;
            }
            parent.ancestors().any(|anc| {
                ast::RecordExpr::can_cast(anc.kind())
                    || ast::RecordPat::can_cast(anc.kind())
                    || ast::TupleStructPat::can_cast(anc.kind())
                    || ast::PathExpr::can_cast(anc.kind())
            })
        })
}
