/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use ra_ap_syntax::{AstNode, SyntaxKind, SyntaxNode, SyntaxToken, ast};

/// Unexpanded macro input is opaque: discovering only RecordExpr nodes would
/// silently skip its constructors. Refuse affected calls until source-mapped
/// expansion is available, including when other ordinary sites were discovered.
pub(super) fn blocked_references(
    root: &SyntaxNode,
    text: &str,
    type_name: &str,
    selected: Option<&SyntaxNode>,
    references: &[(u32, u32)],
    crate_root: &super::rust_crate::CrateRoot,
) -> Vec<String> {
    let referenced = |offset| {
        crate::signature::line_col_at(text, offset)
            .is_some_and(|position| references.contains(&position))
    };
    let mut blocked = Vec::new();
    for tree in root
        .descendants()
        .filter_map(ast::TokenTree::cast)
        .filter(|tree| {
            !tree
                .syntax()
                .ancestors()
                .skip(1)
                .any(|node| ast::TokenTree::can_cast(node.kind()))
        })
    {
        let tokens: Vec<_> = tree
            .syntax()
            .descendants_with_tokens()
            .filter_map(|element| element.into_token())
            .filter(|token| !token.kind().is_trivia())
            .collect();
        for (index, token) in tokens.iter().enumerate() {
            if !matches!(token.kind(), SyntaxKind::IDENT | SyntaxKind::SELF_TYPE_KW)
                || (token.text() != type_name && token.text() != "Self")
                || !could_construct(&tokens[index + 1..])
            {
                continue;
            }
            let offset = usize::from(token.text_range().start());
            if !referenced(offset)
                && !super::rust_identity::opaque_is_selected(
                    tree.syntax(),
                    &path_names(&tokens, index),
                    selected,
                    type_name,
                    crate_root,
                    |path| {
                        path.segment()
                            .and_then(|s| s.name_ref())
                            .is_some_and(|name| {
                                referenced(usize::from(name.syntax().text_range().start()))
                            })
                    },
                )
            {
                continue;
            }
            let (line, col) = crate::signature::position_at(text, offset).unwrap_or((0, 0));
            blocked.push(format!(
                "{line}:{col} opaque macro arguments may reference selected constructor `{type_name}`; source-mapped expansion is required"
            ));
            break;
        }
    }
    blocked
}

fn could_construct(suffix: &[SyntaxToken]) -> bool {
    let mut tokens = suffix.iter().map(|token| token.text());
    match tokens.next() {
        Some("{") => true,
        // Generic constructor input stays opaque; do not attempt to balance
        // expression operators as generic brackets. Ordinary associated calls
        // such as Config::make() are not constructor literals.
        Some("::") => tokens.next() == Some("<"),
        Some(":") => tokens.next() == Some(":") && tokens.next() == Some("<"),
        _ => false,
    }
}

// Token trees may retain `::` as two punctuation tokens. Keep only an ordinary
// path prefix; a generic/opaque prefix will remain unresolved and be refused.
fn path_names(tokens: &[SyntaxToken], mut index: usize) -> Vec<String> {
    let mut names = vec![tokens[index].text().to_string()];
    while index > 0 {
        let separator = if tokens[index - 1].text() == "::" {
            1
        } else if index >= 2 && tokens[index - 1].text() == ":" && tokens[index - 2].text() == ":" {
            2
        } else {
            break;
        };
        if index <= separator {
            break;
        }
        index -= separator + 1;
        if !matches!(
            tokens[index].kind(),
            SyntaxKind::IDENT
                | SyntaxKind::SELF_TYPE_KW
                | SyntaxKind::SELF_KW
                | SyntaxKind::SUPER_KW
                | SyntaxKind::CRATE_KW
        ) {
            // Mark a nontrivial prefix unresolved instead of treating it as bare.
            names.insert(0, String::new());
            break;
        }
        names.insert(0, tokens[index].text().to_string());
    }
    names
}
