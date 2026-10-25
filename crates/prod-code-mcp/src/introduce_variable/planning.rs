/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{
    analysis::occurrences,
    purity::Proof,
    syntax::{Syntax, ident},
};
use anyhow::{Context, Result, ensure};
use std::{net::SocketAddr, ops::Range, path::Path};

pub(super) struct Plan {
    pub replacements: Vec<Range<usize>>,
    pub anchor: usize,
    pub initializer: Range<usize>,
}

pub(super) async fn plan(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    text: &str,
    selection: Range<usize>,
    name: &str,
) -> Result<Plan> {
    let s = Syntax::load(remote, root, file, text).await?;
    let selected = s.expression_at(selection.start, selection.len())
        .context("the selection is not a complete executable Rust expression (macro token semantics are unproven)")?;
    let function = s
        .function(selected)
        .context("the selection is not inside a function")?;
    ensure!(
        !s.ancestors(function).any(|i| s.child(i, "ATTR").is_some()),
        "enclosing attribute expansion has unproven evaluation semantics"
    );
    ensure!(
        !s.nodes
            .iter()
            .enumerate()
            .any(|(i, n)| s.inside(i, function) && n.kind == "ERROR"),
        "cannot establish occurrence identity in malformed Rust syntax"
    );
    // Include reads as well as bindings: introducing a local must not capture an existing
    // use of a parameter, outer local, constant, function, or a name in a macro's tokens.
    // Compare Rust's NFC identifier identity, not its original source spelling.
    let requested = ident(name);
    ensure!(
        !s.nodes
            .iter()
            .any(|n| n.kind == "IDENT" && ident(&text[n.span.clone()]) == requested),
        "requested name `{name}` is not fresh in this source; introducing it could capture or shadow an existing name"
    );

    let expr = &text[selection.clone()];
    let body_node = s
        .child(function, "BLOCK_EXPR")
        .context("function has no executable body")?;
    let body = &s.nodes[body_node].span;
    let found = occurrences(text, body.start, body.end, expr);
    ensure!(
        found.contains(&selection.start),
        "the selection is not an executable occurrence"
    );
    let mut copies = Vec::new();
    for at in found {
        let copy = match s.expression_at(at, expr.len()) {
            Some(copy) => copy,
            None if non_expression_name(&s, at, expr.len()) => continue,
            None => anyhow::bail!(
                "an executable text match is not the same expression syntax; macro token semantics or expression boundaries are unproven"
            ),
        };
        ensure!(
            s.nodes[copy].kind == s.nodes[selected].kind && s.function(copy) == Some(function),
            "an occurrence has a different expression or function identity"
        );
        ensure!(
            !s.ancestors(copy).any(|i| matches!(
                s.nodes[i].kind.as_str(),
                "TOKEN_TREE" | "ATTR" | "CONST_ARG" | "ARRAY_TYPE" | "CONST_BLOCK_EXPR"
            )),
            "an occurrence has unproven macro, attribute or const evaluation semantics"
        );
        copies.push(copy);
    }
    let first = copies[0];
    let last = *copies.last().context("no occurrences")?;
    let block = s
        .ancestors(first)
        .find(|i| s.nodes[*i].kind == "STMT_LIST" && copies.iter().all(|c| s.inside(*c, *i)))
        .context("no shared executable block")?;
    let statement = s
        .ancestors(first)
        .find(|i| s.nodes[*i].parent == Some(block))
        .context("no insertion statement")?;
    ensure!(
        matches!(s.nodes[statement].kind.as_str(), "LET_STMT" | "EXPR_STMT")
            || s.is_expr(statement),
        "cannot prove insertion into this statement kind"
    );
    let anchor = s.nodes[statement].span.start;
    for copy in &copies {
        for (i, n) in s.nodes.iter().enumerate() {
            if n.kind == "PATH_EXPR" && s.inside(i, *copy) {
                s.binding(function, s.code(i))?;
            }
        }
        ensure!(
            !s.ancestors(*copy)
                .take_while(|i| *i != block)
                .any(|i| matches!(
                    s.nodes[i].kind.as_str(),
                    "CLOSURE_EXPR" | "ASYNC_BLOCK_EXPR" | "FN" | "CONST" | "STATIC"
                )),
            "an occurrence crosses a deferred evaluation or item boundary"
        );
    }

    let mut proof = Proof::new(&s, remote, root, file, function);
    for copy in &copies {
        proof.scalar(*copy).await?;
        for (i, n) in s.nodes.iter().enumerate() {
            if n.kind != "PATH_EXPR" || !s.inside(i, *copy) {
                continue;
            }
            let binding = s.binding(function, s.code(i))?;
            let declaration = s.nodes[binding].parent.context("unknown binding scope")?;
            if s.nodes[declaration].kind == "LET_STMT" {
                let scope = s.nodes[declaration].parent.context("unknown local scope")?;
                ensure!(
                    s.ancestors(block).any(|a| a == scope)
                        && s.nodes[declaration].span.end <= anchor,
                    "a dependency is not bound at the insertion point"
                );
            }
        }
    }

    let panics = super::analysis::can_panic(expr);
    if panics {
        ensure!(
            copies.iter().any(|copy| guaranteed(&s, *copy, block)),
            "`{expr}` can panic, and no occurrence runs whenever the binding would; guarded panic/evaluation placement is unproven"
        );
    }
    let end = s.nodes[last].span.end;
    let mut watched: Vec<_> = std::iter::once(anchor..end).collect();
    for (i, n) in s.nodes.iter().enumerate() {
        if matches!(n.kind.as_str(), "FOR_EXPR" | "WHILE_EXPR" | "LOOP_EXPR")
            && n.span.start >= anchor
            && copies.iter().any(|c| s.inside(*c, i))
        {
            watched.push(n.span.clone());
        }
    }
    let in_watch = |i: usize| {
        watched
            .iter()
            .any(|r| r.start <= s.nodes[i].span.start && s.nodes[i].span.end <= r.end)
    };
    // Check entire expressions between evaluations, not the spelling of a receiver followed
    // by `(`. This sees generic method calls, assignments, mutable borrows, overloaded ops,
    // macro expansions, deferred work, and possible drops of nonprimitive temporaries.
    for (i, n) in s.nodes.iter().enumerate() {
        if !in_watch(i) {
            continue;
        }
        ensure!(
            !matches!(
                n.kind.as_str(),
                "RETURN_EXPR"
                    | "BREAK_EXPR"
                    | "CONTINUE_EXPR"
                    | "TRY_EXPR"
                    | "AWAIT_EXPR"
                    | "UNSAFE_KW"
            ),
            "cannot preserve evaluation placement across an exit, suspension or unsafe operation"
        );
        if !s.is_expr(i) || copies.iter().any(|c| s.inside(i, *c)) {
            continue;
        }
        // A call/operator surrounding an earlier copy can run *after* that copy but before
        // the next. Do not exempt it merely because it contains a selected expression.
        let control = matches!(
            n.kind.as_str(),
            "BLOCK_EXPR" | "IF_EXPR" | "MATCH_EXPR" | "FOR_EXPR" | "WHILE_EXPR" | "LOOP_EXPR"
        );
        if control && copies.iter().any(|c| s.inside(*c, i)) {
            continue;
        }
        if n.kind != "LITERAL" {
            proof
                .scalar(i)
                .await
                .context("intervening evaluation may change dependencies or observable effects")?;
        }
    }

    if panics {
        // A textual first copy may be skipped. Until a guaranteed copy executes,
        // alternative branches can run any earlier expression in this prefix. Include
        // those branches rather than treating the first text match as a control-flow cut.
        let frontier = copies
            .iter()
            .copied()
            .find(|copy| guaranteed(&s, *copy, block))
            .context("no guaranteed occurrence for panic placement")?;
        for (i, n) in s.nodes.iter().enumerate() {
            if n.span.start >= anchor
                && n.span.end <= s.nodes[frontier].span.start
                && s.is_expr(i)
                && !copies.iter().any(|c| s.inside(i, *c))
            {
                // An operation's own panic follows its mandatory operands. If one
                // already evaluates the extracted expression, hoisting that expression
                // cannot move it ahead of this panic. Earlier operand expressions are
                // checked separately; conditional copies do not establish this order.
                let follows_copy = copies.iter().any(|copy| {
                    s.ancestors(*copy).any(|ancestor| ancestor == i) && guaranteed(&s, *copy, i)
                });
                ensure!(
                    !may_panic(&s, i) || follows_copy,
                    "an earlier evaluation can panic; extraction would change panic order"
                );
            }
        }
    }

    super::capture::preserve_macro_bindings(&s, remote, root, file, block, anchor, name).await?;

    let replacements = copies
        .iter()
        .map(|copy| {
            let mut outer = *copy;
            while let Some(parent) = s.nodes[outer].parent {
                if s.nodes[parent].kind != "PAREN_EXPR" {
                    break;
                }
                if s.nodes
                    .iter()
                    .enumerate()
                    .any(|(i, n)| n.kind == "COMMENT" && s.inside(i, parent))
                {
                    break;
                }
                outer = parent;
            }
            s.nodes[outer].span.clone()
        })
        .collect::<Vec<_>>();
    ensure!(
        replacements.windows(2).all(|w| w[0].end <= w[1].start),
        "overlapping expression occurrences"
    );
    let mut bare = selected;
    while s.nodes[bare].kind == "PAREN_EXPR" {
        let children = s.expressions(bare);
        ensure!(children.len() == 1, "unknown grouping syntax");
        bare = children[0];
    }
    Ok(Plan {
        replacements,
        anchor,
        initializer: s.nodes[bare].span.clone(),
    })
}

fn may_panic(s: &Syntax<'_>, i: usize) -> bool {
    match s.nodes[i].kind.as_str() {
        "BIN_EXPR" => !matches!(
            s.operator(i).as_str(),
            "==" | "!=" | "<" | "<=" | ">" | ">=" | "&&" | "||" | "&" | "|" | "^"
        ),
        "PREFIX_EXPR" => s.operator(i) == "-",
        "INDEX_EXPR" | "CALL_EXPR" | "METHOD_CALL_EXPR" | "MACRO_EXPR" => true,
        _ => false,
    }
}

fn non_expression_name(s: &Syntax<'_>, at: usize, len: usize) -> bool {
    s.nodes.iter().enumerate().any(|(i, n)| {
        if n.span != (at..at + len) {
            return false;
        }
        // A secondary pattern copy is not evaluated and must retain its bytes.
        // A directly selected pattern was already rejected by expression_at.
        s.in_pattern(i)
            || n.kind == "NAME"
            || n.kind == "NAME_REF"
                && (n.parent.is_some_and(|p| s.nodes[p].kind == "FIELD_EXPR")
                    || s.ancestors(i)
                        .take_while(|p| !s.is_expr(*p))
                        .any(|p| s.nodes[p].kind.ends_with("_TYPE")))
    })
}

fn guaranteed(s: &Syntax<'_>, copy: usize, block: usize) -> bool {
    let mut child = copy;
    for parent in s.ancestors(copy).skip(1).take_while(|i| *i != block) {
        let kind = s.nodes[parent].kind.as_str();
        let conditional = matches!(
            kind,
            "MATCH_ARM"
                | "CLOSURE_EXPR"
                | "FOR_EXPR"
                | "WHILE_EXPR"
                | "LOOP_EXPR"
                | "ASYNC_BLOCK_EXPR"
        );
        if conditional {
            return false;
        }
        if kind == "IF_EXPR" && s.expressions(parent).first().copied() != Some(child) {
            return false;
        }
        if kind == "BIN_EXPR"
            && matches!(s.operator(parent).as_str(), "&&" | "||")
            && s.expressions(parent).first().copied() != Some(child)
        {
            return false;
        }
        child = parent;
    }
    true
}
