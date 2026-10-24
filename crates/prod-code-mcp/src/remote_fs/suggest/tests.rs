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

fn checkout(files: &[&str]) -> tempfile::TempDir {
    let dir = tempfile::Builder::new().prefix("sg").tempdir().unwrap();
    for rel in files {
        let path = dir.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "x").unwrap();
    }
    dir
}

#[test]
fn a_split_module_or_index_file_is_suggested_for_the_flat_path() {
    let dir = checkout(&[
        "src/a/mod.rs",
        "src/b/index.tsx",
        "src/c/index.js",
        "src/d/__init__.py",
        "src/e/index.ts",
        "src/f.rs",
    ]);
    let cases = [
        ("src/a.rs", "src/a/mod.rs"),
        ("src/b.tsx", "src/b/index.tsx"),
        ("src/c.js", "src/c/index.js"),
        ("src/d.py", "src/d/__init__.py"),
        ("src/e.ts", "src/e/index.ts"),
        ("src/f/mod.rs", "src/f.rs"),
    ];
    for (missing, want) in cases {
        assert_eq!(
            suggest_nearest_path(dir.path(), Path::new(missing)).as_deref(),
            Some(want),
            "{missing}"
        );
        let absolute = dir.path().join(missing);
        assert_eq!(
            suggest_nearest_path(dir.path(), &absolute).as_deref(),
            Some(want),
            "absolute {missing}"
        );
    }
}

#[test]
fn a_directory_of_the_same_name_lists_its_first_three_files() {
    let dir = checkout(&[
        "src/m/d.txt",
        "src/m/b.txt",
        "src/m/c.txt",
        "src/m/a.txt",
        "src/m/sub/z.txt",
    ]);
    assert_eq!(
        suggest_nearest_path(dir.path(), Path::new("src/m.rs")).as_deref(),
        Some("src/m/a.txt`, `src/m/b.txt`, `src/m/c.txt")
    );
}

#[test]
fn a_close_sibling_name_is_a_typo_and_a_far_one_is_not() {
    let dir = checkout(&["src/handler.rs", "src/zzzzzzzzzz.rs"]);
    assert_eq!(
        suggest_nearest_path(dir.path(), Path::new("src/handlr.rs")).as_deref(),
        Some("src/handler.rs")
    );
    assert_eq!(
        suggest_nearest_path(dir.path(), Path::new("src/Handler.rs")).as_deref(),
        Some("src/handler.rs")
    );
    assert_eq!(
        suggest_nearest_path(dir.path(), Path::new("src/completely_other.rs")),
        None
    );
    assert_eq!(
        suggest_nearest_path(dir.path(), Path::new("src/handler.rs")),
        None
    );
}

#[test]
fn a_moved_file_is_found_elsewhere_best_overlap_first_and_at_most_three() {
    let dir = checkout(&[
        "crates/b/src/util.rs",
        "crates/a/util.rs",
        "z/util.rs",
        "w/util.rs",
        "pkg/util/mod.rs",
        "target/util.rs",
        ".hidden/util.rs",
        "node_modules/util.rs",
    ]);
    assert_eq!(
        suggest_nearest_path(dir.path(), Path::new("src/old/util.rs")).as_deref(),
        Some("crates/b/src/util.rs`, `crates/a/util.rs`, `w/util.rs")
    );
    let dir = checkout(&["pkg/util/mod.rs", "target/util.rs", ".hidden/util.rs"]);
    assert_eq!(
        suggest_nearest_path(dir.path(), Path::new("old/util.rs")).as_deref(),
        Some("pkg/util/mod.rs")
    );
    let dir = checkout(&["target/util.rs", ".hidden/util.rs", "vendor/util.rs"]);
    assert_eq!(
        suggest_nearest_path(dir.path(), Path::new("old/util.rs")),
        None
    );
}

#[test]
fn missing_file_context_appends_the_hint_only_for_a_missing_path() {
    let dir = checkout(&["src/a/mod.rs", "src/real.rs"]);
    let root = dir.path();
    let real = root.join("src/real.rs");
    assert_eq!(
        missing_file_context(root, &real, "read"),
        format!("read {}", real.display())
    );
    let gone = root.join("src/a.rs");
    assert_eq!(
        missing_file_context(root, &gone, "open"),
        format!("open {} (did you mean `src/a/mod.rs`?)", gone.display())
    );
    let nothing = root.join("elsewhere/none.rs");
    assert_eq!(
        missing_file_context(root, &nothing, "open"),
        format!("open {}", nothing.display())
    );
}

#[test]
fn the_workspace_scan_is_bounded() {
    let dir = checkout(&["sub/util.rs"]);
    for n in 0..=MAX_SUGGEST_SCAN_FILES {
        std::fs::write(dir.path().join(format!("f{n}.txt")), "").unwrap();
    }
    assert_eq!(
        suggest_nearest_path(dir.path(), Path::new("old/util.rs")),
        None
    );
}

#[test]
fn edit_distance_counts_characters_not_bytes() {
    let cases = [
        ("", "", 0),
        ("abc", "", 3),
        ("", "ab", 2),
        ("kitten", "sitting", 3),
        ("é", "e", 1),
        ("日本", "日本語", 1),
    ];
    for (a, b, want) in cases {
        assert_eq!(edit_distance(a, b), want, "{a} {b}");
    }
}
