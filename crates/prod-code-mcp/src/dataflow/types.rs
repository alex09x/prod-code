/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use serde::{Deserialize, Serialize};

/// Completeness contract for intra-function data-flow slicing (Roadmap 7.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SliceCompleteness {
    /// Full data-flow and control-dependency graph resolved with complete evidence.
    Complete,
    /// Bounded by explicit line budget or statement cutoff.
    Bounded {
        max_statements: usize,
        actual_statements: usize,
    },
    /// Incomplete analysis due to unresolvable dynamic scoping, missing targets, or unparseable syntax.
    Incomplete { gap: String },
}

impl SliceCompleteness {
    pub fn is_complete(&self) -> bool {
        matches!(self, SliceCompleteness::Complete)
    }

    pub fn label(&self) -> &'static str {
        match self {
            SliceCompleteness::Complete => "COMPLETE",
            SliceCompleteness::Bounded { .. } => "BOUNDED",
            SliceCompleteness::Incomplete { .. } => "INCOMPLETE",
        }
    }
}

/// One statement in the intra-function slice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SliceStatement {
    /// 1-based line number in original file.
    pub line: u32,
    /// Exact trimmed source line.
    pub text: String,
    /// Leading indentation string for formatted reconstruction.
    pub indent: String,
    /// Reason this statement is included in the slice.
    pub reason: String,
    /// Whether this is a control dependency (e.g. if/while/match condition).
    pub is_control: bool,
    /// Variables defined / mutated by this statement.
    pub defined_vars: Vec<String>,
    /// Variables used / referenced by this statement.
    pub used_vars: Vec<String>,
}

/// Result of intra-function data-flow slicing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataFlowSlice {
    pub function_name: String,
    pub file: String,
    pub start_line: u32,
    pub end_line: u32,
    pub target_line: u32,
    pub target_var: Option<String>,
    pub total_lines: usize,
    pub retained_lines: usize,
    pub reduction_percent: f64,
    pub completeness: SliceCompleteness,
    pub statements: Vec<SliceStatement>,
    pub formatted_slice: String,
}
