/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! In-memory diagnostics for a document (roadmap 7.7): what the analyzer thinks of a file,
//! or of a proposed replacement text, without a build and without writing anything.

pub mod annotate;
pub mod filter;
pub mod ident;
pub mod parse;
pub mod platform;
pub mod reexport;
pub mod stream;
pub mod syntax;
pub mod types;
pub mod validate;

#[cfg(test)]
mod tests;

pub use annotate::*;
pub use filter::*;
pub use ident::*;
pub use parse::*;
pub use platform::*;
pub use reexport::*;
pub use stream::*;
pub use syntax::*;
pub use types::*;
pub use validate::*;
