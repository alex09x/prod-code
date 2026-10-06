/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::signature::Modifiers;
use anyhow::Result;
use std::path::PathBuf;

/// The gopls release whose behaviour this adapter was written and tested against.
pub const GOPLS_VERSION: &str = "v0.23.0";

/// The part of the requirement that a refusal leaves open, so that it stays visible.
pub(crate) const STILL_OPEN: &str = "Go signature changes here are limited to reordering named parameters, \
     removing ones proven unused, and adding explicitly typed primitive parameters with literal \
     arguments to ordinary non-generic functions and named value or pointer receiver methods, plus \
     replacing one unnamed primitive result of an ordinary non-generic free function or named value or pointer receiver method; \
     variadics, generic functions or receivers, combined additions with removals, reorders or type \
     changes, named or multiple results, result removal or addition from void, scope-dependent or \
     composite results, method expressions or values, interface signatures or dispatch, and broader modifiers \
     remain open requirements (#448)";

/// A parameter as the declaration declares it, flattened out of Go's grouping: `a, b int` is two.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoParam {
    pub name: String,
    /// The type as written, in canonical spacing and without comments.
    pub ty: String,
}

/// A function or method declaration, as its header writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Decl {
    /// Byte offset of the `func` keyword: where gopls is asked.
    pub(crate) func_at: usize,
    pub(crate) name: String,
    pub(crate) name_at: usize,
    /// The receiver, canonical, for a method.
    pub(crate) receiver: Option<String>,
    pub(crate) generic: bool,
    /// Offsets of the parentheses around the parameters.
    pub(crate) open: usize,
    pub(crate) close: usize,
    /// The results, canonical; empty when there are none.
    pub(crate) results: String,
}

/// A call of the function: where it is, its parentheses and its arguments as written.
#[derive(Debug, Clone)]
pub(crate) struct Call {
    pub(crate) path: PathBuf,
    pub(crate) at: String,
    pub(crate) open: usize,
    pub(crate) close: usize,
    pub(crate) args: Vec<String>,
}

/// What evaluating an argument can do, as far as its text shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArgKind {
    /// A number, string or rune literal, or a function literal: nothing to evaluate that another
    /// argument can change.
    Literal,
    /// A read of a variable or a field (`x`, `a.b`, `&x`). `true`, `false` and `nil` are here
    /// too: they are predeclared identifiers, not keywords, and a program may declare a variable
    /// of that name (`true := 1`), so their spelling proves nothing.
    Place,
    /// Anything else: a call, a conversion, a receive, an index, an operator.
    Effectful,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Addition {
    /// How many old parameters precede this one.
    pub(crate) boundary: usize,
    pub(crate) name: String,
    pub(crate) ty: String,
    pub(crate) value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Receiver {
    pub(crate) binding: String,
    pub(crate) ty: String,
}

/// A replacement of the byte range `start..end` of a file as it is.
pub(crate) type TextEdit = (usize, usize, String);

/// An error that says why, that nothing was written, and what stays open.
pub(crate) fn refusal(why: String) -> anyhow::Error {
    anyhow::anyhow!("{why}; nothing was written. {STILL_OPEN}")
}

/// Visibility and `async` are not parameters or the narrow result replacement supported here.
pub(crate) fn refuse_non_result_modifiers(modifiers: &Modifiers) -> Result<()> {
    if modifiers.visibility.is_some() {
        return Err(refusal(
            "a Go name is exported by its first letter, not by a modifier; use a rename to \
             change it"
                .to_string(),
        ));
    }
    if modifiers.asyncness.is_some() {
        return Err(refusal(
            "Go functions are neither async nor not".to_string(),
        ));
    }
    Ok(())
}
