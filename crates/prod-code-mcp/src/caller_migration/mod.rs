/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Polyglot caller type-annotation migration for extracted interfaces/traits (Roadmap 7.1.2).
//!
//! When an interface or trait is extracted from a class or struct, callers that only invoke
//! methods belonging to the extracted interface/trait have their type annotations migrated
//! from the concrete type to the interface/trait across the file and workspace.

pub mod mask;
pub mod migrate;
pub(crate) mod safety;
pub(crate) mod scan;
pub mod types;

#[cfg(test)]
mod tests;

pub use mask::lexical_code_mask;
pub use migrate::{migrate_caller_annotations, migrate_callers_in_workspace};
pub use types::CallerMigration;
