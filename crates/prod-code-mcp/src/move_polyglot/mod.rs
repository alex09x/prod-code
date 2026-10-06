/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Polyglot refactor.move across TypeScript/JavaScript, Python, Go, C++, and Swift (Roadmap 7.1.1, Epic #516).
//!
//! Relocates top-level declarations (functions, classes, interfaces, types, structs, enums, constants)
//! into another module, updating declarations, carrying dependencies, and rewriting caller imports
//! across the workspace with full diagnostic pre-validation.

pub mod callers;
pub mod decl;
pub mod execute;
pub mod go_imports;
pub mod header;
pub mod imports;
pub mod specifiers;
pub mod target;

#[cfg(test)]
mod tests;

pub use callers::rewrite_caller_imports;
pub use decl::{find_matching_brace_end, find_polyglot_decl, with_doc_comment_polyglot};
pub use execute::move_item;
pub use imports::{
    carry_imports_polyglot, insert_or_merge_py_import, insert_or_merge_ts_import,
    update_source_imports,
};
pub use specifiers::{
    is_compatible_language_family, is_symbol_used, path_relative_from, python_module_specifier,
    relative_import_specifier,
};
pub use target::{check_target_collision, format_item_for_target};
