/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod enclosing;
pub mod execute;
pub mod hover;
pub mod syntax;
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
