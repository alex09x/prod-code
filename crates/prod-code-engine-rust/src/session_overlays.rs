/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Per-session live buffers layered over the shared base workspace.

use std::collections::HashMap;
use std::path::PathBuf;

/// Per-session live buffers layered over the shared base workspace.
///
/// Several sessions (agent worktrees) share one Salsa database. Each session's `didOpen` /
/// `didChange` / dirty-file sync goes into its own overlay instead of the shared base, and a
/// query first activates its session so the database reflects exactly that session's buffers.
/// `owner` records whose text currently sits in the database for a path; `base` keeps the text
/// the database held before the first overlay touched that path (`None` = file absent).
#[derive(Default)]
pub(crate) struct SessionOverlays {
    pub(crate) sessions: HashMap<u64, HashMap<PathBuf, Option<String>>>,
    pub(crate) owner: HashMap<PathBuf, u64>,
    pub(crate) base: HashMap<PathBuf, Option<String>>,
}
