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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CodeCloneOccurrence {
    pub file: String,
    pub start_line: u32,
    pub end_line: u32,
    pub snippet: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CloneGroup {
    pub id: usize,
    pub clone_type: String,
    pub line_count: usize,
    pub occurrences: Vec<CodeCloneOccurrence>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DuplicationReport {
    pub total_files_scanned: usize,
    pub total_lines_scanned: usize,
    pub total_clone_groups: usize,
    pub duplicated_lines: usize,
    pub duplication_percentage: f32,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub approximate: bool,
    pub groups: Vec<CloneGroup>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct DuplicateOptions {
    pub min_lines: usize,
    pub parameterized: bool,
    pub type3: bool,
    pub max_groups: usize,
}

impl Default for DuplicateOptions {
    fn default() -> Self {
        Self {
            min_lines: 6,
            parameterized: true,
            type3: false,
            max_groups: 20,
        }
    }
}
