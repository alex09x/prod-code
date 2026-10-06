/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! High-performance, in-memory Rust analysis engine for prod-code directly utilizing `ra_ap_ide::AnalysisHost`.

use ra_ap_base_db::salsa::Cancelled;

/// Returns true if the error was caused by Salsa query cancellation (e.g. concurrent mutation).
pub fn is_salsa_cancelled(err: &anyhow::Error) -> bool {
    err.downcast_ref::<Cancelled>().is_some()
        || err.to_string().contains("canceled")
        || err.to_string().contains("cancelled")
}

pub mod config;
pub mod editor;
pub mod engine;
mod load_budget;
pub mod priming;
pub mod proc_macro_farm;
pub(crate) mod session_overlays;
pub mod snapshot;
pub mod types;
pub mod vfs;

#[cfg(test)]
mod tests;

pub use config::{FeatureSelection, ProcMacroServerKind, ProdCodeConfig, RustAnalysisOptions};
pub use engine::RustEngine;
pub use priming::PrimingJob;
pub use snapshot::RustEngineSnapshot;
pub use types::{
    AssistInfo, CallEdge, DefinitionTarget, FileDiagnostic, FileMove, HierarchyItem,
    RefactorOutcome, ReferenceTarget, RewrittenFile, SymbolTarget, WorkspaceSymbol,
};
pub(crate) use vfs::line_col_to_offset;
pub use vfs::{MAX_SAFE_FILE_ID, is_safe_file_id, normalize_vfs_path};
