/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Making a public field private, and every access to it outside its module a method call.

pub mod case;
pub mod polyglot;
pub mod rust;
pub mod types;

#[cfg(test)]
mod tests;

pub use case::{display, field_at_line_col, lowercase_first, to_pascal_case, to_snake_case};
pub(crate) use polyglot::field_position_in_lsp;
pub use polyglot::{
    encapsulate_field_cpp, encapsulate_field_go, encapsulate_field_js, encapsulate_field_py,
    encapsulate_field_swift, encapsulate_field_ts, encapsulate_polyglot, field_position_in_symbols,
    lsp_symbol_position, replace_cpp_unqualified, replace_line_self, replace_line_this,
    replace_line_this_private, rewrite_external_cpp, rewrite_external_go, rewrite_external_py,
    rewrite_external_swift, rewrite_external_ts,
};
pub(crate) use rust::chain_start;
pub use rust::{
    access_at, accessors, encapsulate, field_at, inherent_impl, is_generic, owner_at,
    returns_by_value,
};
pub use types::{Access, COPY, EncapsulatedField, FieldDecl, Language};
