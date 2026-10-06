/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Polyglot extract function refactoring across TypeScript, Python, Go, C++, Swift, and Rust (Roadmap Section 7.1.2).
//!
//! Extracts a code selection into a new function, automatically analyzing free variable captures
//! for input parameters and downstream mutations for return synthesis, replacing the selection
//! with a call, and discovering and replacing identical or structurally parameterized duplicates
//! across the file and workspace.

pub mod codegen;
pub mod duplicates;
pub mod execute;
pub mod inference;
pub mod lang;
pub mod outputs;
pub mod scope;
#[cfg(test)]
mod tests;
pub mod tokenize;
pub mod types;

pub use codegen::{generate_call_replacement, generate_function_code};
pub use duplicates::find_duplicates_in_text;
pub use execute::{apply_edits, extract_function_polyglot};
pub use inference::infer_param_type;
pub use lang::{
    Language, collect_workspace_sources, display, is_candidate_source_file, is_ident, mentions,
};
pub use outputs::{analyze_outputs, reindent_body};
pub use scope::{
    extract_input_variables, find_enclosing_scope, has_complete_expression_boundaries, is_balanced,
};
pub use tokenize::{is_builtin_or_global, is_keyword, tokenize_polyglot};
pub use types::{ExtractedParam, OutputKind, PolyOccurrence, PolyToken, PolyTokenKind};
