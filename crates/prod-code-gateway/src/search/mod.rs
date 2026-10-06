/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod collect;
pub mod graph;
pub mod index;
pub mod parser;
pub mod patterns;
pub mod runner;
pub mod scoring;
pub mod tokenizer;
pub mod types;

pub use graph::{TypedGraph, WorkspaceIndex, category_weight, centrality_score};
pub use index::SearchIndexes;
pub use parser::declarations_in;
pub use runner::run_search;
pub use scoring::first_sentence;
pub use tokenizer::tokenize;
pub use types::*;

#[cfg(test)]
mod tests;
