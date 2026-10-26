/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use ra_ap_syntax::ast::{HasGenericParams, HasLoopBody, HasName};
use ra_ap_syntax::{AstNode, SyntaxNode, TextRange, ast};
use std::borrow::Cow;

#[derive(PartialEq, Eq)]
pub(super) enum Identity {
    Declaration(TextRange),
    Primitive(String),
    ResolutionScope(Vec<TextRange>),
    TraitScope(Vec<TextRange>),
}

/// Compare declaration locations, never spelling or a list of enclosing braces.
/// Unknown imports, expansions and conditional bindings do not prove equality;
/// the selected occurrence itself is retained by the caller in that case.
pub(super) fn identities(expression: &SyntaxNode) -> Option<Vec<Identity>> {
    let mut identities = Vec::new();
    for name in expression.descendants().filter_map(ast::NameRef::cast) {
        let Some(segment) = name.syntax().parent().and_then(ast::PathSegment::cast) else {
            // Receiver identity alone does not identify a trait method. Imports
            // (including anonymous/glob imports) can change method lookup.
            if name
                .syntax()
                .parent()
                .is_some_and(|p| ast::MethodCallExpr::can_cast(p.kind()))
            {
                identities.push(Identity::TraitScope(trait_scope(name.syntax())));
            }
            continue;
        };
        let path = segment.syntax().parent().and_then(ast::Path::cast)?;
        // Identical qualified paths in the exact same lexical context resolve
        // identically. Across contexts, syntax alone does not prove namespace
        // or associated-type identity; do not guess from a same-spelled local.
        if path.qualifier().is_some()
            || path
                .syntax()
                .parent()
                .is_some_and(|p| ast::Path::can_cast(p.kind()))
        {
            identities.push(Identity::ResolutionScope(resolution_scope(name.syntax())));
            continue;
        }
        let value = path
            .syntax()
            .parent()
            .is_some_and(|p| ast::PathExpr::can_cast(p.kind()));
        identities.push(resolve(&name, value)?);
    }
    Some(identities)
}

fn resolution_scope(reference: &SyntaxNode) -> Vec<TextRange> {
    let mut scopes = Vec::new();
    for scope in reference.ancestors() {
        if ast::StmtList::can_cast(scope.kind())
            || ast::ItemList::can_cast(scope.kind())
            || ast::ClosureExpr::can_cast(scope.kind())
            || ast::MatchArm::can_cast(scope.kind())
            || ast::ForExpr::can_cast(scope.kind())
            || ast::IfExpr::can_cast(scope.kind())
            || ast::WhileExpr::can_cast(scope.kind())
            || ast::Item::can_cast(scope.kind())
        {
            scopes.push(scope.text_range());
        }
        // An expansion between the occurrences may introduce a new binding.
        if ast::StmtList::can_cast(scope.kind()) {
            scopes.extend(
                scope
                    .children()
                    .filter(|n| {
                        n.text_range().end() <= reference.text_range().start()
                            && n.descendants().any(|n| ast::MacroCall::can_cast(n.kind()))
                    })
                    .map(|n| n.text_range()),
            );
        }
    }
    scopes
}

fn trait_scope(reference: &SyntaxNode) -> Vec<TextRange> {
    reference
        .ancestors()
        .filter(|n| {
            ast::StmtList::can_cast(n.kind())
                || ast::ItemList::can_cast(n.kind())
                || ast::SourceFile::can_cast(n.kind())
        })
        .flat_map(|scope| {
            scope
                .children()
                .filter(|n| {
                    ast::Use::can_cast(n.kind())
                        || ast::Trait::can_cast(n.kind())
                        || ast::Impl::can_cast(n.kind())
                        || is_scope_macro(n)
                        || ast::ExternCrate::can_cast(n.kind())
                })
                .map(|n| n.text_range())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn is_scope_macro(node: &SyntaxNode) -> bool {
    ast::MacroCall::can_cast(node.kind())
        || ast::MacroExpr::can_cast(node.kind())
        || ast::ExprStmt::cast(node.clone())
            .and_then(|stmt| stmt.expr())
            .is_some_and(|expr| ast::MacroExpr::can_cast(expr.syntax().kind()))
}

// Only comparison keys are normalized. Declaration identities and edit ranges
// continue to refer to the original bytes, including raw/decomposed spellings.
pub(super) fn canonical(name: &str) -> Cow<'_, str> {
    icu_normalizer::ComposingNormalizerBorrowed::new_nfc()
        .normalize(name.strip_prefix("r#").unwrap_or(name))
}

fn pattern(pattern: ast::Pat, name: &str) -> Option<TextRange> {
    pattern
        .syntax()
        .descendants()
        .filter_map(ast::IdentPat::cast)
        .find_map(|p| {
            p.name()
                .filter(|n| canonical(&n.text()) == name)
                .map(|n| n.syntax().text_range())
        })
}

fn parameters(list: ast::ParamList, name: &str) -> Option<TextRange> {
    if name == "self" {
        return list.self_param().map(|p| p.syntax().text_range());
    }
    list.params()
        .filter_map(|p| p.pat())
        .find_map(|p| pattern(p, name))
}

fn declared(node: &SyntaxNode, name: &str, value: bool) -> Option<TextRange> {
    // The name is a direct child; nested bodies cannot supply declarations to
    // their enclosing scope. Import targets are not guessed.
    let eligible = if value {
        ast::Fn::can_cast(node.kind())
            || ast::Const::can_cast(node.kind())
            || ast::Static::can_cast(node.kind())
            || ast::Struct::can_cast(node.kind())
    } else {
        ast::Struct::can_cast(node.kind())
            || ast::Enum::can_cast(node.kind())
            || ast::Union::can_cast(node.kind())
            || ast::TypeAlias::can_cast(node.kind())
    };
    if !eligible {
        return None;
    }
    node.children()
        .find_map(ast::Name::cast)
        .filter(|n| canonical(&n.text()) == name)
        .map(|n| n.syntax().text_range())
}

/// An explicit import cannot affect a name other than the one it introduces.
/// Globs and imports of the requested name still require semantic resolution.
fn imports_name(tree: ast::UseTree, name: &str) -> bool {
    if let Some(list) = tree.use_tree_list() {
        return list.use_trees().any(|tree| imports_name(tree, name));
    }
    if tree.star_token().is_some() {
        return true;
    }
    if let Some(rename) = tree.rename() {
        // `as _` introduces no name.
        return rename
            .syntax()
            .children()
            .find_map(ast::Name::cast)
            .is_some_and(|alias| canonical(&alias.text()) == name);
    }
    let Some(segment) = tree.path().and_then(|path| path.segment()) else {
        return true;
    };
    let text = segment.syntax().text().to_string();
    if text == "self" {
        // `use path::module::{self, Other}` introduces `module`, not `self`.
        return tree
            .syntax()
            .ancestors()
            .skip(1)
            .find_map(ast::UseTree::cast)
            .and_then(|parent| parent.path())
            .and_then(|path| path.segment())
            .is_none_or(|segment| canonical(&segment.syntax().text().to_string()) == name);
    }
    canonical(&text) == name
}

pub(super) fn resolve(reference: &ast::NameRef, value: bool) -> Option<Identity> {
    let text = reference.text();
    let key = canonical(&text);
    let name = key.as_ref();
    let at = reference.syntax().text_range().start();
    for scope in reference.syntax().ancestors().skip(1) {
        if value
            && let Some(closure) = ast::ClosureExpr::cast(scope.clone())
            && let Some(binding) = closure.param_list().and_then(|p| parameters(p, name))
        {
            return Some(Identity::Declaration(binding));
        }
        if value
            && let Some(arm) = ast::MatchArm::cast(scope.clone())
            && let Some(binding) = arm.pat().and_then(|p| pattern(p, name))
        {
            return Some(Identity::Declaration(binding));
        }
        if value
            && let Some(loop_) = ast::ForExpr::cast(scope.clone())
            && loop_
                .loop_body()
                .is_some_and(|b| b.syntax().text_range().contains(at))
            && let Some(binding) = loop_.pat().and_then(|p| pattern(p, name))
        {
            return Some(Identity::Declaration(binding));
        }
        if value && (ast::IfExpr::can_cast(scope.kind()) || ast::WhileExpr::can_cast(scope.kind()))
        {
            let condition = scope.children().find_map(ast::Expr::cast)?;
            if condition
                .syntax()
                .descendants()
                .filter_map(ast::LetExpr::cast)
                .filter_map(|l| l.pat())
                .any(|p| pattern(p, name).is_some())
            {
                return None;
            }
        }
        if value && ast::StmtList::can_cast(scope.kind()) {
            // A let binding begins after its complete initializer/else clause.
            // In particular `let base = base * 8` still refers to the older base.
            for statement in scope.children().collect::<Vec<_>>().into_iter().rev() {
                if statement.text_range().end() > at {
                    continue;
                }
                if let Some(binding) = ast::LetStmt::cast(statement.clone())
                    .and_then(|l| l.pat())
                    .and_then(|p| pattern(p, name))
                {
                    return Some(Identity::Declaration(binding));
                }
                // A preceding macro can introduce a binding hidden from this AST.
                if statement
                    .descendants()
                    .any(|n| ast::MacroCall::can_cast(n.kind()))
                {
                    return None;
                }
            }
        }
        if ast::StmtList::can_cast(scope.kind())
            || ast::ItemList::can_cast(scope.kind())
            || ast::SourceFile::can_cast(scope.kind())
        {
            let mut found = None;
            let mut imported = false;
            for item in scope.children() {
                if is_scope_macro(&item) {
                    return None;
                }
                if !ast::Item::can_cast(item.kind()) {
                    continue;
                }
                if ast::Use::cast(item.clone())
                    .is_some_and(|u| u.use_tree().is_none_or(|tree| imports_name(tree, name)))
                {
                    imported = true;
                }
                if let Some(binding) = declared(&item, name, value)
                    && found.replace(binding).is_some()
                {
                    return None;
                }
            }
            if imported {
                if found.is_some() {
                    return None;
                }
                return Some(Identity::ResolutionScope(resolution_scope(
                    reference.syntax(),
                )));
            }
            if let Some(binding) = found {
                return Some(Identity::Declaration(binding));
            }
        }
        if let Some(function) = ast::Fn::cast(scope.clone()) {
            if value && let Some(binding) = function.param_list().and_then(|p| parameters(p, name))
            {
                return Some(Identity::Declaration(binding));
            }
            // Nested items cannot capture a surrounding function's local variables.
            // Generic names require type/namespace resolution rather than guessing.
            if function.generic_param_list().is_some() {
                return None;
            }
        }
        if name == "Self" && ast::Impl::can_cast(scope.kind()) {
            return Some(Identity::Declaration(scope.text_range()));
        }
        if let Some(implementation) = ast::Impl::cast(scope)
            && implementation.generic_param_list().is_some()
        {
            return None;
        }
    }
    matches!(
        name,
        "bool"
            | "char"
            | "str"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "usize"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "isize"
            | "f32"
            | "f64"
    )
    .then(|| Identity::Primitive(name.to_string()))
}
