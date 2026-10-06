/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::BTreeSet;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletedFunction {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Copy)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone)]
pub struct FunctionRange {
    pub name: String,
    pub start: usize,
    pub end: usize,
    pub name_start: usize,
    pub name_end: usize,
}

pub struct Project {
    pub root: PathBuf,
    pub sources: BTreeSet<PathBuf>,
}

pub struct CompileVerdict {
    pub passed: bool,
    pub output: String,
}
