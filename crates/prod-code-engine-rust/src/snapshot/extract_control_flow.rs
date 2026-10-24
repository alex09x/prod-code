/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Refuse loop exits that the extract-function assist cannot transport to its caller.

use ra_ap_syntax::{AstNode, SyntaxKind, SyntaxNode, TextRange, TextSize, ast};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Break,
    Continue,
    Return,
    Try,
}

struct Exit {
    kind: Kind,
    at: TextSize,
    construct: String,
    outer_label: bool,
    nested_return: Option<&'static str>,
}

fn is_loop(node: &SyntaxNode) -> bool {
    matches!(
        node.kind(),
        SyntaxKind::FOR_EXPR | SyntaxKind::WHILE_EXPR | SyntaxKind::LOOP_EXPR
    )
}

fn block_has(node: &SyntaxNode, keyword: &str) -> bool {
    node.kind() == SyntaxKind::BLOCK_EXPR
        && node
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .any(|t| t.text() == keyword)
}

fn function_boundary(node: &SyntaxNode) -> bool {
    matches!(node.kind(), SyntaxKind::FN | SyntaxKind::CLOSURE_EXPR)
        || block_has(node, "async")
        || block_has(node, "gen")
}

fn exit(node: SyntaxNode, selection: TextRange) -> Option<Exit> {
    let (kind, keyword) = match node.kind() {
        SyntaxKind::BREAK_EXPR => (Kind::Break, "break"),
        SyntaxKind::CONTINUE_EXPR => (Kind::Continue, "continue"),
        SyntaxKind::RETURN_EXPR => (Kind::Return, "return"),
        SyntaxKind::TRY_EXPR => (Kind::Try, "?"),
        _ => return None,
    };
    let label = node
        .children()
        .find_map(ast::Lifetime::cast)
        .map(|l| l.syntax().text().to_string());
    let mut nearest_loop = None;
    let mut target = None;
    for parent in node.ancestors().skip(1) {
        if is_loop(&parent) && nearest_loop.is_none() {
            nearest_loop = Some(parent.clone());
        }
        let binds = match kind {
            Kind::Break | Kind::Continue => {
                if function_boundary(&parent) {
                    break;
                }
                if let Some(label) = &label {
                    parent.children().filter_map(ast::Label::cast).any(|l| {
                        l.syntax()
                            .children()
                            .find_map(ast::Lifetime::cast)
                            .is_some_and(|l| l.syntax().text().to_string() == *label)
                    })
                } else {
                    is_loop(&parent)
                }
            }
            Kind::Return => function_boundary(&parent),
            Kind::Try => function_boundary(&parent) || block_has(&parent, "try"),
        };
        if binds {
            target = Some(parent);
            break;
        }
    }
    let target = target?;
    let nested_return = if selection.contains_range(target.text_range()) {
        // The assist transports outer loop exits by rewriting returns, including returns
        // owned by nested callables. Do not let that rewrite corrupt their return types.
        if kind != Kind::Return {
            return None;
        }
        Some(match target.kind() {
            SyntaxKind::FN => "a nested function",
            SyntaxKind::CLOSURE_EXPR => "a closure",
            _ => "an async or generator block",
        })
    } else {
        None
    };
    let outer_label = label.is_some() && nearest_loop.as_ref() != Some(&target);
    let construct = label.map_or_else(|| keyword.to_string(), |l| format!("{keyword} {l}"));
    Some(Exit {
        kind,
        at: if kind == Kind::Try {
            node.text_range().end() - TextSize::from(1)
        } else {
            node.text_range().start()
        },
        construct,
        outer_label,
        nested_return,
    })
}

pub(super) fn refusal(root: &SyntaxNode, selection: TextRange) -> Option<String> {
    let mut exits: Vec<_> = root
        .descendants()
        .filter(|node| selection.contains_range(node.text_range()))
        .filter_map(|node| exit(node, selection))
        .collect();
    exits.sort_by_key(|exit| exit.at);
    let text = root.text().to_string();
    let describe = |exit: &Exit| {
        let line = text[..usize::from(exit.at)]
            .bytes()
            .filter(|b| *b == b'\n')
            .count()
            + 1;
        format!("`{}` at line {line}", exit.construct)
    };
    if let Some(exit) = exits.iter().find(|exit| exit.outer_label) {
        return Some(format!(
            "cannot extract function: {} targets an outer loop or labelled block outside the selection; \
             rust-analyzer cannot preserve this labelled exit. Select its target together with the body \
             or move the exit outside the selection.",
            describe(exit)
        ));
    }
    let jump = exits
        .iter()
        .find(|exit| matches!(exit.kind, Kind::Break | Kind::Continue))?;
    if let Some(exit) = exits.iter().find(|exit| exit.nested_return.is_some()) {
        return Some(format!(
            "cannot extract function: {} belongs to {} inside the selection; rust-analyzer \
             cannot preserve this nested return while transporting loop exits. Move the nested \
             callable outside the selection or select the enclosing loop.",
            describe(exit),
            exit.nested_return.unwrap()
        ));
    }
    if exits.iter().all(|exit| exit.kind == jump.kind) {
        return None;
    }
    Some(format!(
        "cannot extract function: {} cross the selection boundary; extracting mixed loop exits \
         is not supported. Select the enclosing loop or move the exits outside the selection.",
        exits.iter().map(describe).collect::<Vec<_>>().join(", ")
    ))
}
