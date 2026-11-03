/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The declaration side of a borrow conversion: which parameters the request borrows, whether
//! the function can take them by shared reference, and the edit to its parameter list.

use anyhow::{Context, Result};
use ra_ap_syntax::{AstNode, Edition, SourceFile, SyntaxKind, ast, ast::HasAttrs};

use crate::signature::types::{Declared, Param};

/// An insertion into a file: the byte offset, and the text that goes there.
pub type Insertion = (usize, String);

/// The declared parameters to borrow, by index. A borrow keeps the parameter list as it is: every
/// declared parameter is listed once, in its declared order, with `&` in front of those to borrow.
/// Adding, removing or reordering parameters in the same request is refused, so that a borrow is
/// never read as the old remove-and-add of a parameter with the same name.
pub fn borrowed_indices(declared: &[Declared], request: &[Param]) -> Result<Vec<usize>> {
    let names = declared
        .iter()
        .map(|d| d.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    for want in request {
        if let Param::Borrow(name) = want {
            anyhow::ensure!(
                declared.iter().any(|d| &d.name == name),
                "no parameter named `{name}` to borrow; the declaration takes {names}. Nothing \
                 was written"
            );
        }
    }
    let shape = || {
        format!(
            "a borrow keeps the parameter list as it is: list every declared parameter once, in \
             its declared order ({names}), with `&` in front of each one to borrow. Add, remove \
             or reorder parameters in a separate request; nothing was written"
        )
    };
    anyhow::ensure!(request.len() == declared.len(), "{}", shape());
    let mut borrowed = Vec::new();
    for (i, (want, have)) in request.iter().zip(declared).enumerate() {
        match want {
            Param::Keep(name) if *name == have.name => {}
            Param::Borrow(name) if *name == have.name => borrowed.push(i),
            _ => anyhow::bail!("{}", shape()),
        }
    }
    Ok(borrowed)
}

/// The insertions that make each borrowed parameter's type `T` into `&T` (`&(T)` for a type a
/// `&` would split, `impl A + B`), for the function whose parameter list's inside starts at
/// `open`. Refuses a function whose signature is a contract other code shares (a trait's, an
/// `extern` one), a parameter that is not a plain immutable binding of an owned type, and a
/// result that borrows through lifetime elision, which a new reference input would redirect.
pub fn declaration_edits(
    text: &str,
    open: usize,
    name: &str,
    borrowed: &[usize],
) -> Result<Vec<Insertion>> {
    let parsed = SourceFile::parse(text, Edition::Edition2024);
    let list = parsed
        .syntax_node()
        .descendants()
        .filter_map(ast::ParamList::cast)
        .find(|l| usize::from(l.syntax().text_range().start()) + 1 == open)
        .with_context(|| {
            format!("`{name}`'s parameter list could not be read as Rust; nothing was written")
        })?;
    let func = list
        .syntax()
        .parent()
        .and_then(ast::Fn::cast)
        .with_context(|| {
            format!(
                "`{name}` is not a function declaration (a closure or a function type has no \
                 callers to update); nothing was written"
            )
        })?;
    ensure_own_contract(&func, name)?;
    let params: Vec<ast::Param> = list.params().collect();
    let mut edits = Vec::new();
    for &i in borrowed {
        let param = params.get(i).with_context(|| {
            format!(
                "`{name}`'s parameters could not be matched to its declaration; nothing was written"
            )
        })?;
        let shown = param.syntax().text().to_string();
        anyhow::ensure!(
            param.attrs().next().is_none(),
            "`{shown}` has an attribute; borrowing an attributed parameter is not supported. \
             Nothing was written"
        );
        match param.pat() {
            Some(ast::Pat::IdentPat(p))
                if p.mut_token().is_none() && p.ref_token().is_none() && p.at_token().is_none() => {
            }
            Some(ast::Pat::IdentPat(p)) if p.mut_token().is_some() => anyhow::bail!(
                "`{shown}` is a mutable binding: the body may change or move it, which a shared \
                 borrow cannot do. Nothing was written"
            ),
            _ => anyhow::bail!(
                "`{shown}` binds a pattern; only a plain parameter name can be borrowed. Nothing \
                 was written"
            ),
        }
        let ty = param
            .ty()
            .with_context(|| format!("`{shown}` has no type; nothing was written"))?;
        match &ty {
            ast::Type::RefType(_) => anyhow::bail!(
                "`{shown}` is already a reference; only an owned parameter can be borrowed. \
                 Nothing was written"
            ),
            ast::Type::InferType(_) | ast::Type::NeverType(_) | ast::Type::MacroType(_) => {
                anyhow::bail!(
                    "`{shown}`'s type cannot be borrowed by this change; nothing was written"
                )
            }
            _ => {}
        }
        let range = ty.syntax().text_range();
        let (start, end) = (usize::from(range.start()), usize::from(range.end()));
        let split = matches!(ty, ast::Type::ImplTraitType(_) | ast::Type::DynTraitType(_))
            && text[start..end].contains('+');
        if split {
            edits.push((start, "&(".to_string()));
            edits.push((end, ")".to_string()));
        } else {
            edits.push((start, "&".to_string()));
        }
    }
    ensure_no_elided_result(&func, &list, name)?;
    Ok(edits)
}

/// Refuses a function whose signature other declarations must match: a trait's method, a trait
/// implementation's method, and a function with a foreign ABI.
fn ensure_own_contract(func: &ast::Fn, name: &str) -> Result<()> {
    anyhow::ensure!(
        func.abi().is_none(),
        "`{name}` is declared with an ABI (`extern`); its parameter types are part of a foreign \
         interface and are not changed. Nothing was written"
    );
    let owner = func.syntax().parent().and_then(|items| items.parent());
    if let Some(owner) = owner {
        anyhow::ensure!(
            owner.kind() != SyntaxKind::TRAIT && owner.kind() != SyntaxKind::EXTERN_BLOCK,
            "`{name}` is declared in a trait or an `extern` block; every implementation shares \
             its signature, and borrowing one parameter of one of them is not supported. Nothing \
             was written"
        );
        if let Some(imp) = ast::Impl::cast(owner) {
            anyhow::ensure!(
                imp.trait_().is_none(),
                "`{name}` implements a trait method; its signature must match the trait's, and \
                 borrowing one parameter of one implementation is not supported. Nothing was \
                 written"
            );
        }
    }
    Ok(())
}

/// Refuses a result type that borrows through lifetime elision (`-> &str`, `-> Item<'_>`) when no
/// `&self` decides it: a new reference input would change what the result borrows from, or make
/// the elision ambiguous.
fn ensure_no_elided_result(func: &ast::Fn, list: &ast::ParamList, name: &str) -> Result<()> {
    let by_reference = list
        .self_param()
        .is_some_and(|s| s.amp_token().is_some() || matches!(s.ty(), Some(ast::Type::RefType(_))));
    let Some(ret) = func.ret_type().and_then(|r| r.ty()) else {
        return Ok(());
    };
    if by_reference {
        return Ok(());
    }
    let top = ret.syntax().clone();
    // A reference inside a function type or a `Fn(..)` bound has its own elision scope.
    let own_scope = |node: &ra_ap_syntax::SyntaxNode| {
        node.ancestors().take_while(|a| *a != top).any(|a| {
            matches!(
                a.kind(),
                SyntaxKind::FN_PTR_TYPE
                    | SyntaxKind::PARENTHESIZED_ARG_LIST
                    | SyntaxKind::PARAM_LIST
                    | SyntaxKind::RET_TYPE
            )
        })
    };
    let elided = top.descendants().any(|node| {
        let elided_ref = ast::RefType::cast(node.clone()).is_some_and(|r| r.lifetime().is_none());
        let anonymous =
            ast::Lifetime::cast(node.clone()).is_some_and(|l| l.syntax().text() == "'_");
        (elided_ref || anonymous) && !own_scope(&node)
    });
    anyhow::ensure!(
        !elided,
        "`{name}` returns `{}`, which borrows through lifetime elision; a new reference \
         parameter would change what the result borrows from. Name the lifetime first. Nothing \
         was written",
        ret.syntax().text()
    );
    Ok(())
}
