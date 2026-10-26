/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

mod binding_imports;
mod binding_path;
pub mod execute;
mod helper_path;
mod method_syntax;
mod module_attributes;
mod occurrence_path;
mod occurrences;
pub mod parse;
pub mod restructure;
pub mod types;

pub use execute::extract_delegate_rust;
pub use parse::{argument_names, impl_blocks, parse_struct};
pub use restructure::{restructure, rewrite_literals};
pub use types::{Extracted, Field, StructDecl};
