/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Replace constructor / raw struct instantiations with named static factory methods or a fluent builder pattern (Roadmap 7.1.3).

pub mod codegen;
pub mod execute;
pub mod parse;
pub mod rewrite;
pub mod sites;
pub mod types;

#[cfg(test)]
mod tests;

pub use codegen::{generate_builder_code, generate_factory_code};
pub use execute::{
    replace_constructor_impl, replace_constructor_with_builder, replace_constructor_with_factory,
};
pub use parse::{
    extract_generics, parse_go_struct_decl, parse_go_struct_fields, parse_python_fields,
    parse_python_struct_decl, parse_rust_struct_decl, parse_rust_struct_fields,
    parse_struct_declaration, parse_ts_fields, parse_ts_struct_decl, split_balanced_commas,
};
pub use rewrite::rewrite_instantiation;
pub use sites::{
    find_cpp_instantiations, find_go_instantiations, find_python_instantiations,
    find_rust_instantiations, find_swift_instantiations, find_ts_instantiations,
};
pub use types::{FieldDecl, InstantiationSite, ReplaceConstructorResult, ReplaceMode, StructDecl};
