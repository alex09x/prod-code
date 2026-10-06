/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod apply;
pub mod binding;
pub mod c_cpp;
pub mod c_cpp_decls;
pub mod container;
pub mod cpp_std;
pub mod drop;
pub mod effects;
pub mod introduce;
pub mod params;
pub mod polyglot;
pub mod polyglot_callers;
pub mod rewrite;
pub mod rust_introduce;
pub mod rust_types;
pub mod syntax;
pub mod type_render;
pub mod types;

#[cfg(test)]
mod tests;

pub use binding::{bind_arguments, call_args_span};
pub use drop::{
    declared_async, drop_glue, drop_order, dropped_differently, name_in, no_drop_glue, rust_code,
    rust_edition, spelled,
};
pub use effects::{js_constant, reordered_arguments};
pub use introduce::introduce;
pub use params::{entries, parse_params, split_default};
pub use rewrite::{rewritten_args, rewritten_call};
pub use rust_types::{needs_lifetime, parameter_text, struct_text, type_of, with_lifetime};
pub use syntax::{close_in, is_ident_byte, matching_bracket, split_args};
pub use type_render::{aggregate_text, hover_parameter_type, literal_text, type_text};
pub use types::{
    CDeclaration, Field, Kind, Language, Param, ParameterObject, Spelled, default_binding,
};
