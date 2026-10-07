/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::heuristics::web_config::has_starlark_project;
use super::super::*;
use prod_code_protocol::messages::EngineKind;
use tempfile::tempdir;

#[test]
fn test_starlark_detection_ignores_workspace_and_build_directories() {
    let dir = tempdir().unwrap();
    // Directories named workspace or build must not be detected as Starlark
    std::fs::create_dir_all(dir.path().join("workspace")).unwrap();
    std::fs::create_dir_all(dir.path().join("build")).unwrap();
    assert!(!has_starlark_project(dir.path()));
    assert_ne!(detect_engine(dir.path()), EngineKind::Starlark);

    // Lowercase files named workspace or build must not trigger Starlark
    let dir2 = tempdir().unwrap();
    std::fs::write(dir2.path().join("workspace"), "# script\n").unwrap();
    std::fs::write(dir2.path().join("build"), "# script\n").unwrap();
    assert!(!has_starlark_project(dir2.path()));
    assert_ne!(detect_engine(dir2.path()), EngineKind::Starlark);

    // Uppercase files named WORKSPACE or BUILD do trigger Starlark
    let dir3 = tempdir().unwrap();
    std::fs::write(dir3.path().join("WORKSPACE"), "").unwrap();
    assert!(has_starlark_project(dir3.path()));
    assert_eq!(detect_engine(dir3.path()), EngineKind::Starlark);

    let dir4 = tempdir().unwrap();
    std::fs::write(dir4.path().join("BUILD"), "").unwrap();
    assert!(has_starlark_project(dir4.path()));
    assert_eq!(detect_engine(dir4.path()), EngineKind::Starlark);
}
