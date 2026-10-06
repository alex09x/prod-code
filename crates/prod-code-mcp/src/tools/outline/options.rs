/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

/// What an outline lists, and how much of it (#368).
#[derive(Debug, Clone)]
pub struct OutlineOptions {
    /// The deepest nesting listed, 1 = top-level items only.
    pub max_depth: usize,
    /// Also the local variables inside function bodies.
    pub include_locals: bool,
    /// How to ask for the locals, for the line that says they were left out.
    pub hint: String,
    /// Only these kinds, as the outline names them (`function`, `struct`, ...).
    pub kinds: Option<Vec<String>>,
    /// Only what the language exports (see `is_exported`).
    pub exported_only: bool,
    /// The most bytes of outline listed. A directory's outline names the files after it
    /// instead of outlining them; a file's is cut after the symbols that fit.
    pub max_bytes: Option<usize>,
    /// The most symbols listed.
    pub max_items: Option<usize>,
}

impl OutlineOptions {
    /// Everything, to the depth given, with no budget.
    pub fn all(max_depth: usize, include_locals: bool, hint: &str) -> Self {
        Self {
            max_depth,
            include_locals,
            hint: hint.to_string(),
            kinds: None,
            exported_only: false,
            max_bytes: None,
            max_items: None,
        }
    }
}

/// The budget of a directory's outline when none is asked for: about ten thousand tokens. One
/// Go package's full outline was 79 KB and 1,667 symbols (#368).
pub const DIRECTORY_OUTLINE_BYTES: usize = 40_000;
