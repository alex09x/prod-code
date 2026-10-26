/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

mod c_arrays;
mod c_bindings;
mod c_constants;
mod c_declarations;
mod c_templates;
mod contexts;
pub mod enclosing;
pub mod execute;
mod expressions;
mod go_bindings;
mod go_types;
pub mod hover;
mod javascript;
mod literals;
mod occurrences;
mod regions;
pub mod syntax;
mod tokens;
pub mod types;

#[cfg(test)]
mod tests;

pub use enclosing::{
    enclosing_declaration, enclosing_function, unreported_note, with_argument, with_parameter,
};
pub use execute::extract;
pub use hover::type_from_hover;
pub use syntax::Syntax;
pub use types::{Enclosing, ExtractedParameter};
