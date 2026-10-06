/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod across;
pub mod casing;
pub mod detect;
pub mod edits;
pub mod execute;
pub mod scan;
pub mod types;

#[cfg(test)]
mod tests;

pub use across::rename_across;
pub use casing::variants;
pub use edits::{
    make_workspace_edit, make_workspace_edit_with_ends, make_workspace_edit_with_lines,
};
pub use execute::rename;
pub use types::{AcrossRepos, SchemaRename, Variant, lsp_end_position};
