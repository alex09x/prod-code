/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;

#[test]
fn mutations_preserve_final_newline_and_report_invalid_target_lines() {
    let dir = tempfile::tempdir().unwrap();
    let target = DivergentTarget {
        language: Language::Rust,
        file_rel: "lib.rs".into(),
        symbol: "sample".into(),
        line: 0,
    };
    for ending in ["", "\n"] {
        std::fs::write(
            dir.path().join("lib.rs"),
            format!("pub fn sample() {{}}{ending}"),
        )
        .unwrap();
        let result = apply_mutation(dir.path(), WorktreeKind::SignatureChange, &target).unwrap();
        assert_eq!(result, (dir.path().join("lib.rs"), "sample".into()));
        assert_eq!(
            std::fs::read_to_string(&result.0).unwrap(),
            format!("pub fn sample(divergent_marker: i64) {{}}{ending}")
        );
    }
    std::fs::write(dir.path().join("lib.rs"), "").unwrap();
    assert_eq!(
        apply_mutation(dir.path(), WorktreeKind::SignatureChange, &target)
            .unwrap_err()
            .to_string(),
        "target line 0 out of range"
    );
    let target = DivergentTarget {
        language: Language::Go,
        ..target
    };
    std::fs::write(dir.path().join("lib.rs"), "func sample() {}\n").unwrap();
    assert_eq!(
        apply_mutation(dir.path(), WorktreeKind::UntrackedFile, &target)
            .unwrap_err()
            .to_string(),
        format!("no package clause in {:?}", dir.path().join("lib.rs"))
    );
}

#[test]
fn detects_manifests_and_refuses_reusing_an_origin() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        detect_language(dir.path()).unwrap_err().to_string(),
        format!(
            "no Cargo.toml or go.mod at {:?}; the divergent benchmark needs a Rust or Go repository root",
            dir.path()
        )
    );
    std::fs::write(dir.path().join("go.mod"), "module fixture\n").unwrap();
    assert_eq!(detect_language(dir.path()).unwrap(), Language::Go);
    std::fs::write(dir.path().join("Cargo.toml"), "").unwrap();
    assert_eq!(detect_language(dir.path()).unwrap(), Language::Rust);
    std::fs::create_dir(dir.path().join("origin")).unwrap();
    assert_eq!(
        prepare_origin(None, dir.path(), "origin")
            .unwrap_err()
            .to_string(),
        format!(
            "origin path {:?} already exists; use a fresh workdir",
            dir.path().join("origin")
        )
    );
}

#[cfg(unix)]
#[test]
fn invalid_utf8_repository_and_worktree_paths_are_refused() {
    use std::os::unix::ffi::OsStrExt;
    let invalid = Path::new(std::ffi::OsStr::from_bytes(b"invalid-\xff"));
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        prepare_origin(Some(invalid), dir.path(), "origin")
            .unwrap_err()
            .to_string(),
        format!("base repo path must be valid UTF-8: {invalid:?}")
    );
    let invalid_dir = dir.path().join(invalid);
    let target = invalid_dir.join("origin");
    assert_eq!(
        prepare_origin(Some(dir.path()), &invalid_dir, "origin")
            .unwrap_err()
            .to_string(),
        format!("workdir path must be valid UTF-8: {target:?}")
    );
    assert_eq!(
        create_worktree(dir.path(), &invalid_dir, WorktreeKind::Master, 0)
            .unwrap_err()
            .to_string(),
        format!(
            "worktree path must be valid UTF-8: {:?}",
            invalid_dir.join("wt-master")
        )
    );
}
