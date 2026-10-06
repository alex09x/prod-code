/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Code clone and duplication harvester (roadmap 9.3).

pub mod detect;
pub(crate) mod dsu;
pub(crate) mod normalize;
pub mod render;
pub(crate) mod type3;
pub mod types;

#[cfg(test)]
mod tests;

pub use detect::find_duplicates;
pub use render::format_duplication_report;
pub use types::{CloneGroup, CodeCloneOccurrence, DuplicateOptions, DuplicationReport};
