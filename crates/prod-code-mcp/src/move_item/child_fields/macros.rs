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
use std::path::Path;

use anyhow::{Context, Result};
use ra_ap_syntax::{AstNode, Edition, SourceFile, SyntaxKind, SyntaxNode, SyntaxToken, ast};

use super::{NeededFields, PrivateFields, canonical_ident};
use crate::move_item::item::display;
use crate::move_item::rust_syntax;

const EXPR_MACROS: &[&str] = &[
    "assert",
    "assert_eq",
    "assert_ne",
    "debug_assert",
    "debug_assert_eq",
    "debug_assert_ne",
    "vec",
    "dbg",
    "println",
    "eprintln",
    "format",
    "write",
    "writeln",
];

pub(super) fn inside_token_tree(node: &SyntaxNode) -> bool {
    node.ancestors()
        .skip(1)
        .any(|a| ast::TokenTree::can_cast(a.kind()))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn check_macro_trees(
    root: &Path,
    scope_path: &Path,
    text: &str,
    tree: &SourceFile,
    in_scope: &impl Fn(usize) -> bool,
    ref_coords: &BTreeSet<(u32, u32)>,
    is_self_of_s: &impl Fn(&SyntaxNode) -> bool,
    struct_name: &str,
    edition: Edition,
    shape: &PrivateFields,
    needed: &mut NeededFields,
) -> Result<()> {
    for tt in tree
        .syntax()
        .descendants()
        .filter_map(ast::TokenTree::cast)
        .filter(|tt| !inside_token_tree(tt.syntax()))
    {
        if !in_scope(rust_syntax::range(tt.syntax()).start) {
            continue;
        }
        let tokens: Vec<SyntaxToken> = tt
            .syntax()
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .filter(|t| !t.kind().is_trivia())
            .collect();
        for (idx, tok) in tokens.iter().enumerate() {
            let offset = usize::from(tok.text_range().start());
            let is_ref = crate::signature::line_col_at(text, offset)
                .is_some_and(|lc| ref_coords.contains(&lc));
            let is_self = tok.kind() == SyntaxKind::SELF_TYPE_KW && is_self_of_s(tt.syntax());
            if !is_ref && !is_self {
                continue;
            }
            let (line, col) = crate::signature::position_at(text, offset).unwrap_or((1, 1));
            let site = format!("{}:{line}:{col}", display(root, scope_path));
            if tt
                .syntax()
                .ancestors()
                .any(|a| ast::MacroRules::can_cast(a.kind()) || ast::MacroDef::can_cast(a.kind()))
            {
                anyhow::bail!(
                    "cannot prove minimum field visibility for `{struct_name}` referenced inside a \
                     macro definition at {site}; nothing was written"
                );
            }
            let Some(mac_name) = enclosing_macro_name(&tokens, idx, tt.syntax()) else {
                anyhow::bail!(
                    "cannot prove minimum field visibility for `{struct_name}` inside macro tokens \
                     at {site}; nothing was written"
                );
            };
            handle_macro_struct_mention(
                &mac_name,
                &tokens,
                idx,
                &tt,
                &site,
                struct_name,
                edition,
                shape,
                needed,
            )?;
        }
    }
    Ok(())
}

fn enclosing_macro_name(
    tokens: &[SyntaxToken],
    idx: usize,
    outer_tt: &SyntaxNode,
) -> Option<String> {
    let mut current = tokens[idx].parent();
    while let Some(node) = current {
        if &node == outer_tt {
            break;
        }
        if ast::TokenTree::can_cast(node.kind()) {
            let start = node.text_range().start();
            if let Some(open_pos) = tokens.iter().position(|t| t.text_range().start() == start)
                && open_pos >= 2
                && tokens[open_pos - 1].text() == "!"
                && tokens[open_pos - 2].kind() == SyntaxKind::IDENT
            {
                return Some(tokens[open_pos - 2].text().to_string());
            }
        }
        current = node.parent();
    }
    outer_tt
        .ancestors()
        .find_map(ast::MacroCall::cast)?
        .path()?
        .segments()
        .last()?
        .name_ref()
        .map(|nr| nr.text().to_string())
}

#[allow(clippy::too_many_arguments)]
fn handle_macro_struct_mention(
    mac_name: &str,
    tokens: &[SyntaxToken],
    idx: usize,
    outer_tt: &ast::TokenTree,
    site: &str,
    struct_name: &str,
    edition: Edition,
    shape: &PrivateFields,
    needed: &mut NeededFields,
) -> Result<()> {
    let suffix = &tokens[idx + 1..];
    if let Some(first) = suffix.first() {
        let double_colon = first.text() == "::"
            || (first.text() == ":" && suffix.get(1).is_some_and(|s| s.text() == ":"));
        if double_colon {
            let after_cc = if first.text() == "::" { 1 } else { 2 };
            if suffix.get(after_cc).is_some_and(|t| t.text() == "<") {
                anyhow::bail!(
                    "cannot prove minimum field visibility for `{struct_name}` with turbofish \
                     inside `{mac_name}!` at {site}; nothing was written"
                );
            }
            return Ok(());
        }
    }
    if mac_name == "offset_of" {
        if suffix.first().is_some_and(|t| t.text() == ",")
            && let Some(field_tok) = suffix.get(1)
        {
            match shape {
                PrivateFields::Record { unique_names, .. }
                    if field_tok.kind() == SyntaxKind::IDENT =>
                {
                    let ident = canonical_ident(field_tok.text());
                    if unique_names.contains(&ident) {
                        needed.record_names.insert(ident);
                    }
                    return Ok(());
                }
                PrivateFields::Tuple { fields } if field_tok.kind() == SyntaxKind::INT_NUMBER => {
                    if let Ok(i) = field_tok.text().parse::<usize>() {
                        if fields.iter().any(|f| f.index == i) {
                            needed.tuple_indices.insert(i);
                        }
                        return Ok(());
                    }
                }
                _ => {}
            }
        }
        anyhow::bail!(
            "cannot prove minimum field visibility for `{struct_name}` inside `offset_of!` at \
             {site}; nothing was written"
        );
    }
    if mac_name != "matches" && !EXPR_MACROS.contains(&mac_name) {
        anyhow::bail!(
            "cannot prove minimum field visibility for `{struct_name}` referenced in macro \
             `{mac_name}!` at {site}; nothing was written"
        );
    }
    match shape {
        PrivateFields::Record { unique_names, .. } => {
            let Some(next) = suffix.first() else {
                return Ok(());
            };
            if next.text() != "{" {
                return Ok(());
            }
            let brace_start = next.text_range().start();
            let brace_tt = outer_tt
                .syntax()
                .descendants()
                .filter_map(ast::TokenTree::cast)
                .find(|t| t.syntax().text_range().start() == brace_start)
                .context("unbalanced braces in macro invocation")?;
            let braced = brace_tt.syntax().text().to_string();
            let as_pat = SourceFile::parse(
                &format!("fn _p() {{ match () {{ Self {braced} => {{}} }} }}"),
                edition,
            );
            if as_pat.errors().is_empty()
                && let Some(pat) = as_pat
                    .tree()
                    .syntax()
                    .descendants()
                    .find_map(ast::RecordPat::cast)
                && let Some(list) = pat.record_pat_field_list()
            {
                if list.rest_pat().is_some() {
                    anyhow::ensure!(
                        mac_name == "matches",
                        "cannot prove minimum field visibility for `{struct_name}` with `..` \
                         inside `{mac_name}!` at {site}; nothing was written"
                    );
                    for f in list.fields() {
                        if let Some(name) = f.field_name() {
                            let ident = canonical_ident(name.text());
                            if unique_names.contains(&ident) {
                                needed.record_names.insert(ident);
                            }
                        }
                    }
                } else {
                    needed.mark_all(shape);
                }
                return Ok(());
            }
            let as_expr =
                SourceFile::parse(&format!("fn _e() {{ let _ = Self {braced}; }}"), edition);
            if as_expr.errors().is_empty() {
                needed.mark_all(shape);
                return Ok(());
            }
            anyhow::bail!(
                "cannot prove minimum field visibility for `{struct_name}` inside `{mac_name}!` \
                 at {site}; nothing was written"
            );
        }
        PrivateFields::Tuple { .. } => {
            let prev = idx.checked_sub(1).and_then(|p| tokens.get(p));
            let is_type_arg = prev.is_some_and(|p| p.text() == "<" || p.text() == ":")
                && suffix
                    .first()
                    .is_some_and(|n| matches!(n.text(), ">" | "," | "=" | ";"));
            if !is_type_arg {
                needed.mark_all(shape);
            }
            Ok(())
        }
    }
}
