/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Applying gateway refactorings (LSP `WorkspaceEdit`) to the local checkout. Rewritten files
//! are dropped from the sync watermark so the next pre-flight uploads them.

pub mod edits;
pub mod engine;
pub mod execute;
pub mod history;
pub mod journal;
pub mod locations;
pub mod ops;
#[cfg(test)]
mod tests;
pub mod uri;

pub use edits::{
    apply_scalar_text_edits, apply_text_edits, planned_multi_texts, planned_texts, read_existing,
    text_for_edit,
};
pub use execute::{apply_multi_repository_workspace_edit, apply_workspace_edit};
pub use history::{remember_applied, remember_applied_multi, text_before_apply};
pub use locations::{lsp_locations, referenced_text};
pub use ops::{MultiOp, check_multi_ops, multi_operations};
pub use uri::{contained, resolve, uri_to_relative, uri_to_root_and_relative};
