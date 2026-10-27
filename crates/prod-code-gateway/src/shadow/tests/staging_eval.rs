/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::{Path, PathBuf};

use super::super::staging::{dir_name, safe_relative, stage_upper};
use super::fixtures::delta;

#[test]
fn safe_relative_rejects_absolute_and_parent_paths() {
    assert_eq!(
        safe_relative("src/lib.rs"),
        Some(PathBuf::from("src/lib.rs"))
    );
    assert_eq!(
        safe_relative("./src/lib.rs"),
        Some(PathBuf::from("src/lib.rs"))
    );
    assert_eq!(safe_relative("/etc/passwd"), None);
    assert_eq!(safe_relative("../x"), None);
    assert_eq!(safe_relative("src/../../x"), None);
    assert_eq!(safe_relative(""), None);
    // Control characters would split or corrupt the overlay's line-based deletion list.
    for bad in [
        "a\nb",
        "name\n../outside",
        "a\rb",
        "a\tb",
        "a\0b",
        "a\u{7f}",
        "a\u{85}",
    ] {
        assert_eq!(safe_relative(bad), None, "{bad:?}");
    }
    for good in [
        "dir with space/ünï cödé ✓.txt",
        " lead ",
        "back\\slash",
        "新しい.rs",
    ] {
        assert_eq!(safe_relative(good), Some(PathBuf::from(good)), "{good:?}");
    }
}

#[test]
fn stage_upper_writes_files_and_lists_deletions() {
    let upper = tempfile::tempdir().unwrap();
    let deleted = stage_upper(
        upper.path(),
        &[
            delta("crates/x/src/lib.rs", Some("pub fn x() {}\n")),
            delta("old.rs", None),
        ],
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(upper.path().join("crates/x/src/lib.rs")).unwrap(),
        "pub fn x() {}\n"
    );
    assert_eq!(deleted, vec![PathBuf::from("old.rs")]);
    assert!(stage_upper(upper.path(), &[delta("../escape", Some(""))]).is_err());
}

#[test]
fn dir_names_are_filesystem_safe() {
    let name = dir_name(Path::new("/srv/ws/repo--wt-1"), "fix: use/slice", 255);
    assert_eq!(name, ".prod-code-hypothesis-repo--wt-1--fix__use_slice-ff");
}

/// #1466: names that `dir_name` makes alike, punctuation or a shared 48-character prefix, get
/// directories of their own, each hypothesis having a nonce of its own.
#[test]
fn alike_hypothesis_names_get_their_own_directories() {
    use super::super::staging::unique_nonce;
    let ws = Path::new("/srv/ws/repo");
    let long = "x".repeat(60);
    let names = [
        dir_name(ws, "fix/a", unique_nonce()),
        dir_name(ws, "fix:a", unique_nonce()),
        dir_name(ws, &format!("{long}1"), unique_nonce()),
        dir_name(ws, &format!("{long}2"), unique_nonce()),
    ];
    let distinct: std::collections::BTreeSet<_> = names.iter().collect();
    assert_eq!(distinct.len(), names.len(), "{names:?}");
}

/// #1466: a hypothesis whose directory is already there does not stage into it or remove it.
#[tokio::test]
async fn a_taken_hypothesis_directory_is_neither_used_nor_removed() {
    if let Some(reason) = super::super::root::overlay_unavailable() {
        eprintln!("skipped: {reason}");
        return;
    }
    let ws = tempfile::tempdir().unwrap();
    std::fs::write(ws.path().join("a.txt"), "base\n").unwrap();
    let shadow = tempfile::tempdir().unwrap();
    let mut j = super::fixtures::job(
        ws.path(),
        shadow.path(),
        "fix/a",
        vec![delta("a.txt", Some("proposed\n"))],
        &["true"],
    );
    j.nonce = 42;
    let taken = shadow.path().join(dir_name(ws.path(), "fix:a", 42));
    std::fs::create_dir_all(&taken).unwrap();
    std::fs::write(taken.join("theirs"), "keep\n").unwrap();
    let (_tx, rx) = tokio::sync::watch::channel(false);
    let r = super::super::overlay::run_overlay(j, rx).await;
    assert!(
        r.error
            .as_deref()
            .is_some_and(|e| e.contains("already in use")),
        "{:?}",
        r.error
    );
    assert_eq!(
        std::fs::read_to_string(taken.join("theirs")).unwrap(),
        "keep\n"
    );
}
