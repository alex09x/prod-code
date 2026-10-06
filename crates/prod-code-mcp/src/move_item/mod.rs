/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Moving a declaration into another module, with the imports that keep it compiling.
//!
//! The analyzer says where the symbol is declared and where it is used. This module decides
//! what the text should become — the item cut from one file and pasted into another, the `use`
//! statements rewritten, the qualified paths requalified — and the whole change is type-checked
//! in one overlay before a byte is written, the same shape as [`crate::signature`] and
//! [`crate::schema`].

mod execute;
mod imports;
mod item;
mod module;
#[cfg(test)]
mod tests;
mod types;

pub(crate) use execute::document_symbols;
pub use execute::move_item;
pub use imports::{add_import, carry_imports, drop_import, requalify};
pub use item::{append_item, cut, span_at, with_doc_comment};
pub use module::{declare_module, module_of, parent_module_file};
pub use types::{ModulePath, Move};
