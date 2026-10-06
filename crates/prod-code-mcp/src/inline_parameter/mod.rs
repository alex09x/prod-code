/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Inlining a parameter: when every call passes the same constant for it, the value moves into
//! the body as a local binding, and the parameter leaves the declaration and every call.
//!
//! `fn clamp(x: u32, max: u32)` called as `clamp(v, LIMIT)` everywhere becomes `fn clamp(x: u32)`
//! with `let max: u32 = LIMIT;` at the top of its body, and the calls become `clamp(v)`. The
//! value must mean the same thing in the body as at the call: a literal, a constant, a path. A
//! lowercase name may be a local of the caller and is refused, and so is a set of calls that do
//! not agree on the value. Supports Rust, TypeScript, JavaScript, Python, C++, Swift, and Go.

pub mod calls;
pub mod decl;
pub mod polyglot;
pub mod rust;
pub mod syntax;
pub mod types;

#[cfg(test)]
mod tests;

pub use calls::find_calls_in_content;
pub use decl::{
    extract_decl_name_from_line, find_polyglot_declaration, find_python_body_close, format_binding,
};
pub use polyglot::inline_parameter_polyglot;
pub use rust::inline_parameter;
pub use syntax::{
    is_c_cpp_prototype, is_caller_independent, is_import_or_export_context, is_in_comment,
    is_in_string, keyword_arg, language_matches, swift_label,
};
pub use types::{FoundCall, InlinedParameter, PolyglotDecl};
