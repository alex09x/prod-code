/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Polyglot structural AST search and rewrite engine.
//!
//! Provides pattern matching with metavariables across supported languages.

pub mod execute;
pub mod matcher;
pub mod pattern;
pub mod scope;
pub mod source;
pub mod types;

#[cfg(test)]
mod tests;

pub use execute::{run_codemod, run_structural_search};
pub use matcher::{find_structural_matches_in_source, rewrite_source};
pub use scope::resolve_workspace_scope;
pub use source::{tokenize_source, tokenize_source_for_lang};
pub use types::{
    CODE_EXTENSIONS, CodemodMatch, CodemodOutcome, CodemodRule, CompiledPattern, PatternToken,
    ReplacementToken, SourceToken, StructuralMatchItem, StructuralSearchResult, TokenKind,
    byte_to_line_col,
};
