/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use ra_ap_syntax::ast::HasName;
use ra_ap_syntax::{AstNode, SyntaxNode, ast};

use super::bindings::{Identity, canonical, resolve};

/// Selection already requests a self access in its enclosing closures and async
/// blocks. A new capture elsewhere needs independent evidence that it cannot
/// escape or be coerced into a capture-free function. Declaration equality is
/// not that evidence: a captured Copy local can outlive the method's borrowed self.
pub(super) fn compatible(selected: &SyntaxNode, candidate: &SyntaxNode) -> bool {
    candidate.ancestors().all(|ancestor| {
        if let Some(closure) = ast::ClosureExpr::cast(ancestor.clone()) {
            return selected.ancestors().any(|n| n == *closure.syntax())
                || locally_consumed(closure.syntax(), is_callee);
        }
        if let Some(block) = ast::BlockExpr::cast(ancestor)
            && is_capture_block(&block)
        {
            return selected.ancestors().any(|n| n == *block.syntax())
                || (is_async_block(&block) && locally_consumed(block.syntax(), is_awaited));
        }
        true
    })
}

fn is_capture_block(block: &ast::BlockExpr) -> bool {
    block.async_token().is_some() || block.gen_token().is_some() || block.move_token().is_some()
}

fn is_async_block(block: &ast::BlockExpr) -> bool {
    block.async_token().is_some()
        && block.gen_token().is_none()
        && block.try_block_modifier().is_none()
}

fn unparenthesized_parent(node: &SyntaxNode) -> Option<(SyntaxNode, SyntaxNode)> {
    let mut value = node.clone();
    while let Some(parent) = value.parent() {
        if !ast::ParenExpr::can_cast(parent.kind()) {
            return Some((value, parent));
        }
        value = parent;
    }
    None
}

fn is_callee(value: &SyntaxNode, parent: &SyntaxNode) -> bool {
    ast::CallExpr::cast(parent.clone())
        .and_then(|call| call.expr())
        .is_some_and(|callee| callee.syntax() == value)
}

fn is_awaited(value: &SyntaxNode, parent: &SyntaxNode) -> bool {
    ast::AwaitExpr::cast(parent.clone())
        .and_then(|awaited| awaited.expr())
        .is_some_and(|expr| expr.syntax() == value)
}

fn locally_consumed(boundary: &SyntaxNode, consumed: fn(&SyntaxNode, &SyntaxNode) -> bool) -> bool {
    let Some(function) = boundary.ancestors().find_map(ast::Fn::cast) else {
        return false;
    };
    // A shared self borrow can be copied into a local move closure or awaited
    // future. Moving an owned self or introducing a mutable borrow needs borrow analysis.
    let Some(receiver) = function.param_list().and_then(|p| p.self_param()) else {
        return false;
    };
    if receiver.amp_token().is_none() || receiver.mut_token().is_some() {
        return false;
    }
    let Some((value, parent)) = unparenthesized_parent(boundary) else {
        return false;
    };
    if consumed(&value, &parent) {
        return true;
    }
    let Some(binding) = ast::LetStmt::cast(parent) else {
        return false;
    };
    if binding.ty().is_some()
        || binding
            .initializer()
            .is_none_or(|init| init.syntax() != &value)
    {
        return false;
    }
    let Some(pattern) = binding
        .pat()
        .and_then(|p| ast::IdentPat::cast(p.syntax().clone()))
    else {
        return false;
    };
    // `ref f` and `f @ ...` change the binding's shape and call requirements.
    if pattern.ref_token().is_some() || pattern.pat().is_some() {
        return false;
    }
    let Some(name) = pattern.name() else {
        return false;
    };
    let Some(scope) = binding.syntax().parent().and_then(ast::StmtList::cast) else {
        return false;
    };
    // Token-tree uses cannot be classified as direct calls with the linked AST.
    if scope
        .syntax()
        .descendants()
        .any(|n| ast::MacroCall::can_cast(n.kind()))
    {
        return false;
    }
    let declaration = name.syntax().text_range();
    let name_text = name.text();
    let name_key = canonical(&name_text);
    for reference in scope.syntax().descendants().filter_map(ast::NameRef::cast) {
        if canonical(&reference.text()) != name_key
            || !reference
                .syntax()
                .parent()
                .is_some_and(|p| ast::PathSegment::can_cast(p.kind()))
        {
            continue;
        }
        match resolve(&reference, true) {
            Some(Identity::Declaration(found)) if found == declaration => {}
            Some(Identity::Declaration(_)) => continue,
            _ => return false,
        }
        // Passing, returning, casting or capturing the local is not a proven
        // local consumption, even when another use consumes it directly.
        if reference
            .syntax()
            .ancestors()
            .take_while(|n| n != scope.syntax())
            .any(|n| {
                ast::ClosureExpr::can_cast(n.kind())
                    || ast::BlockExpr::cast(n.clone()).is_some_and(|b| is_capture_block(&b))
                    || ast::Item::can_cast(n.kind())
            })
        {
            return false;
        }
        let Some(path) = reference
            .syntax()
            .parent()
            .and_then(|segment| segment.parent())
            .and_then(|path| path.parent())
            .and_then(ast::PathExpr::cast)
        else {
            return false;
        };
        if !unparenthesized_parent(path.syntax())
            .is_some_and(|(value, parent)| consumed(&value, &parent))
        {
            return false;
        }
    }
    true
}
