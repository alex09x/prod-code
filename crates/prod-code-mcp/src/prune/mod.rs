/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Pruning orphans (roadmap 8.6): every function and type the dead-code scan finds unreferenced
//! is removed with the analyzer's safe delete, all in one edit that is type-checked before
//! anything is written.

pub mod edits;
pub mod execute;
pub mod git;
pub mod types;

#[cfg(test)]
mod tests;

pub use edits::{merge, minimal_edits, text_edits};
pub use execute::{prune_orphans, prune_orphans_opts};
pub use git::create_prune_commit;
pub use types::Pruned;
