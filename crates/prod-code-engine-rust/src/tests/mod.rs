/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Unit tests and test fixture helpers for prod-code-engine-rust.

use std::path::PathBuf;

mod config;
mod diagnostics;
mod engine;
mod navigation;
mod refactoring;
mod sessions;
mod vfs;
mod worktrees;

pub(crate) fn create_test_fixture() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let cargo_toml = r#"[package]
name = "fixture"
version = "0.1.0"
edition = "2021"
"#;
    std::fs::write(temp.path().join("Cargo.toml"), cargo_toml).unwrap();
    let src_dir = temp.path().join("src");
    std::fs::create_dir_all(&src_dir).unwrap();
    let code = r#"pub const DEFAULT_PORT: u16 = 9400;

pub struct PathTranslator {
    pub prefix: String,
}

impl PathTranslator {
    pub fn new(prefix: &str) -> Self {
        Self { prefix: prefix.to_string() }
    }
}
"#;
    let lib_path = src_dir.join("lib.rs");
    std::fs::write(&lib_path, code).unwrap();
    (temp, lib_path)
}
