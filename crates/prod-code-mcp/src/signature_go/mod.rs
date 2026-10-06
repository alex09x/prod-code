/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod add_params;
pub mod add_plan;
pub mod edits;
pub mod evidence;
pub mod execute;
pub mod hazards;
pub mod interface;
pub mod parse;
pub mod reorder;
pub mod reorder_verify;
pub mod result;
pub mod shadowing;
pub mod syntax;
pub mod text;
pub mod types;

#[cfg(test)]
mod tests;

pub const GOPLS_VERSION: &str = types::GOPLS_VERSION;
pub(crate) use evidence::receiver_interface_evidence;
pub use execute::change_with;
pub(crate) use interface::package_interface_method_file;
pub use types::GoParam;
