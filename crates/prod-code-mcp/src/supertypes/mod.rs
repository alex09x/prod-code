/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Supertypes (roadmap 7.5): what a type implements, and what a trait requires. The other
//! direction, what implements a trait, is `code_implementations`.
//!
//! rust-analyzer has no LSP type hierarchy, so for Rust the answer is read from what it does
//! have. For a trait, the supertraits are the bounds after the colon in its own header. For a
//! type, the derived traits are read from the `#[derive(…)]` attributes above it, and the
//! written ones from its implementations (`textDocument/implementation` on the type): an
//! `impl Trait for Type` header gives `Trait`, and an inherent `impl Type` is not a supertype.
//! rust-analyzer reports a derive among the implementations too, at the attribute for a
//! built-in derive and at the type's own name for a macro such as serde's, which is why derives
//! are read from the attributes instead.
//!
//! The other languages' servers are asked for their own type hierarchy
//! (`textDocument/prepareTypeHierarchy`, then `typeHierarchy/supertypes`). A server without one
//! gets that said instead of an empty list.

pub mod resolve;
pub(crate) mod rust;
pub mod syntax;
pub mod types;
pub(crate) mod validate;

#[cfg(test)]
mod tests;

pub use resolve::supertypes;
pub use syntax::{impl_trait, supertraits};
pub use types::{Kind, MAX_DEPTH, Supertype, Supertypes};
