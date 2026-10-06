/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod execute;
pub mod parse;
pub mod restructure;
pub mod types;

pub use execute::extract_delegate_rust;
pub use parse::{argument_names, impl_blocks, parse_struct};
pub use restructure::{restructure, rewrite_literals};
pub use types::{Extracted, Field, StructDecl};
