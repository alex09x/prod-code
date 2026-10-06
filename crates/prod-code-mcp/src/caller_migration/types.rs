/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallerMigration {
    pub var_name: String,
    pub original_type: String,
    pub new_type: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ReplacementCandidate {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) replacement: String,
    pub(crate) migration: CallerMigration,
}

pub(crate) fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}
