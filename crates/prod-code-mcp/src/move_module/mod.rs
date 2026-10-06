/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Moving a whole module to another parent: `a::b` becomes `c::b`, with its file and its
//! submodules.

pub mod execute;
pub mod syntax;
pub mod types;

#[cfg(test)]
mod tests;

pub use execute::move_module;
pub use syntax::{Spelling, declaration, declare_block, is_ident, resolve_super, spelling};
pub use types::ModuleMove;
