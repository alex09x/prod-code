/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::PathBuf;

/// One entry of the requested parameter list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Param {
    /// Keep the parameter declared under this name, in this position.
    Keep(String),
    /// Add a parameter, passing `value` at every call site.
    Add {
        name: String,
        ty: String,
        value: String,
    },
}

/// A reference to the symbol: file, line, column, all 1-based.
pub type Reference = (PathBuf, u32, u32);

/// A parameter as the declaration writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declared {
    /// The whole parameter, `name: Type` with any `mut` or pattern kept verbatim.
    pub raw: String,
    /// What the parameter is called, for matching against the request.
    pub name: String,
}

/// What a change did, or would do.
#[derive(Debug)]
pub struct SignatureChange {
    pub symbol: String,
    /// The workspace root, so that the report can show relative paths.
    pub root: PathBuf,
    /// The declaring file, relative to the workspace root.
    pub file: String,
    pub old_signature: String,
    pub new_signature: String,
    /// The structural rule the call-site rewrite ran, empty when the order did not change.
    pub rule: String,
    /// Every file this changes, as (relative path, whole new content).
    pub rewritten: Vec<(String, String)>,
    /// References the analyzer knows about that the rewrite did not touch.
    pub unmatched: Vec<String>,
    /// Lines the rewrite changed that are not references — an over-match, or a call the
    /// analyzer did not list.
    pub unexpected: Vec<String>,
    /// Errors the analyzer reports for the changed files, checked together.
    pub diagnostics: Vec<String>,
    pub applied: bool,
    /// The return type before and after, when the request changed it (`()` for none).
    pub returns: Option<(String, String)>,
    /// The visibility before and after, when the request changed it (`private` for none).
    pub visibility: Option<(String, String)>,
    /// Whether it was `async` and is now, when the request changed it.
    pub asyncness: Option<(bool, bool)>,
    /// Calls that now `.await` from a function that is not `async`: each blocks the write
    /// unless `force`.
    pub not_async: Vec<String>,
}

/// What a signature change does besides the parameters: the return type and the visibility.
#[derive(Debug, Clone, Default)]
pub struct Modifiers {
    /// The return type the function should have; `()` removes it.
    pub returns: Option<String>,
    /// `pub`, `pub(crate)`, `pub(super)`, `pub(in path)`, or `private` to remove it.
    pub visibility: Option<String>,
    /// Whether the function should be `async`; every call gains or loses its `.await`.
    pub asyncness: Option<bool>,
}

#[derive(Debug)]
pub struct Plan {
    pub list: Vec<String>,
    pub args: Vec<Option<usize>>,
    pub dropped: Vec<String>,
}

/// A call of the function, in the text before the rewrite.
#[derive(Debug)]
pub struct OldCall {
    pub place: String,
    /// See [`call_start`].
    pub start: usize,
    /// Where the argument list opens.
    pub open: usize,
    /// Just past its `)`, or past the `.await` on it.
    pub end: usize,
    /// Whether the rewrite has to change it.
    pub needs: bool,
}

/// A call of the function: where, for the report, and its arguments as written.
#[derive(Debug)]
pub struct CallSite {
    pub at: String,
    pub args: Vec<String>,
}

/// What evaluating an argument can do, as far as its text and the analyzer show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgKind {
    /// A literal: nothing to evaluate, and nothing another argument does changes it.
    Literal,
    /// A read of a local, a constant or a static (`x`, `a::B`, `&mut x`, `x as u64`) passed where
    /// no conversion can run code: no effect of its own, but another argument's effect can change
    /// what it reads.
    Place,
    /// Anything else: a call, a macro, an operator, an index, `?`, a block — it may do
    /// something, or panic.
    Effectful,
    /// It looks like a read, but it can run code the text does not show, and nothing rules that
    /// out; the reason.
    Unproven(&'static str),
}

/// Why a field read is not a plain read.
pub const FIELD_DEREF: &str = "reading a field calls a user `Deref` when the value's own type does \
                               not have that field";
/// Why an argument for a reference parameter is not a plain read.
pub const REF_COERCION: &str = "an argument for a reference parameter can be converted by a user \
                                `Deref` (`&Wrapper` passed for `&Inner`)";
/// Why an argument for a parameter whose type is not known is not a plain read.
pub const UNCONFIRMED_TYPE: &str = "the analyzer does not confirm that the parameter's type is a \
                                    built-in scalar, a struct, an enum or a union, which no conversion \
                                    into runs code";

/// What the analyzer confirmed about a parameter's type.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParamFacts {
    /// Dropping a value of it runs no code.
    pub drop_free: bool,
    /// No conversion of an argument into it runs code.
    pub coercion_free: bool,
    /// It is written as a reference, the type `Deref` coercion converts to.
    pub reference: bool,
    /// Names the type would be drop-free by if they were the built-in types, which the analyzer
    /// did not confirm they are.
    pub unconfirmed: Vec<String>,
}
