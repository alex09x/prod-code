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
