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

#[path = "tests/deletion_tests.rs"]
mod deletion;

#[test]
fn a_hunk_is_applied_where_it_says_or_where_its_lines_moved() {
    let text = "a\nb\nc\nd\ne\n";
    let diff = "--- a/f.rs\n+++ b/f.rs\n@@ -2,2 +2,2 @@\n b\n-c\n+C\n";
    let files = parse(diff).unwrap();
    assert_eq!(
        apply_hunks(text, &files[0].hunks, "f.rs").unwrap(),
        "a\nb\nC\nd\ne\n"
    );
    // Two lines were added at the top since the diff was made.
    let moved = "x\ny\na\nb\nc\nd\ne\n";
    assert_eq!(
        apply_hunks(moved, &files[0].hunks, "f.rs").unwrap(),
        "x\ny\na\nb\nC\nd\ne\n"
    );
    let err = apply_hunks("a\nb\nq\n", &files[0].hunks, "f.rs").unwrap_err();
    assert!(
        format!("{err}").contains("hunk 1 of f.rs does not apply"),
        "{err}"
    );
}

/// A blank line after the diff, as a shell or an editor adds it, is not a context line of
/// the last hunk (#322): it made the parser panic, and would have made the hunk miss. An
/// empty context line inside a hunk stays one.
#[test]
fn blank_lines_after_the_diff_are_not_the_last_hunks() {
    let text = "a\n\nb\nc\n";
    let diff = "--- a/f.rs\n+++ b/f.rs\n@@ -1,3 +1,3 @@\n a\n\n-b\n+B\n\n\n";
    let files = parse(diff).unwrap();
    assert_eq!(
        files[0].hunks[0].lines.len(),
        4,
        "{:?}",
        files[0].hunks[0].lines
    );
    assert_eq!(
        apply_hunks(text, &files[0].hunks, "f.rs").unwrap(),
        "a\n\nB\nc\n"
    );
}

#[test]
fn several_hunks_follow_each_other_and_the_last_newline_is_kept_or_dropped() {
    let text = "1\n2\n3\n4\n5\n6\n";
    let diff = "--- a/f\n+++ b/f\n@@ -1,1 +1,2 @@\n 1\n+1.5\n@@ -5,2 +6,2 @@\n 5\n-6\n+six\n\\ No newline at end of file\n";
    let files = parse(diff).unwrap();
    assert_eq!(
        apply_hunks(text, &files[0].hunks, "f").unwrap(),
        "1\n1.5\n2\n3\n4\n5\nsix"
    );
}

#[test]
fn a_new_file_and_a_deleted_one_are_named_and_paths_lose_their_prefix() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("gone.rs"), "x\n").unwrap();
    std::fs::write(dir.path().join("kept.rs"), "fn a() {}\n").unwrap();
    let diff = "diff --git a/new.rs b/new.rs\n--- /dev/null\n+++ b/new.rs\n@@ -0,0 +1,2 @@\n+pub fn n() {}\n+\n--- a/gone.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-x\n--- a/kept.rs\t2026-09-23\n+++ b/kept.rs\n@@ -1 +1 @@\n-fn a() {}\n+fn a() -> u8 { 1 }\n";
    let patched = apply(dir.path(), diff).unwrap();
    assert_eq!(
        patched.texts,
        vec![
            (dir.path().join("new.rs"), "pub fn n() {}\n\n".to_string()),
            (
                dir.path().join("kept.rs"),
                "fn a() -> u8 { 1 }\n".to_string()
            ),
        ]
    );
    assert_eq!(patched.deleted, vec![dir.path().join("gone.rs")]);
    assert!(apply(dir.path(), "no diff here").is_err());
    assert!(parse("@@ -1 +1 @@\n-a\n+b\n").is_err());
    assert!(parse("--- a/x\nnot plus\n").is_err());
    assert_eq!(diff_path("/dev/null"), None);
    assert_eq!(old_start("@@ -12,5 +12,6 @@ fn x"), Some(12));
    assert_eq!(hunk_counts("@@ -12,5 +12,6 @@ fn x"), Some((5, 6)));
    assert_eq!(hunk_counts("@@ -1 +0,0 @@"), Some((1, 0)));
    assert_eq!(hunk_counts("@@ -1 +1 @@"), Some((1, 1)));
}

#[test]
fn git_pre_image_fallback_when_working_tree_already_has_hunk() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    let _ = std::process::Command::new("git")
        .args(["init"])
        .current_dir(repo)
        .output();
    let _ = std::process::Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(repo)
        .output();
    let _ = std::process::Command::new("git")
        .args(["config", "user.email", "test@test.codes"])
        .current_dir(repo)
        .output();
    std::fs::write(repo.join("file.txt"), "line 1\nline 2 (old)\nline 3\n").unwrap();
    let _ = std::process::Command::new("git")
        .args(["add", "file.txt"])
        .current_dir(repo)
        .output();
    let _ = std::process::Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(repo)
        .output();
    std::fs::write(repo.join("file.txt"), "line 1\nline 2 (new)\nline 3\n").unwrap();
    let diff = "--- a/file.txt\n+++ b/file.txt\n@@ -1,3 +1,3 @@\n line 1\n-line 2 (old)\n+line 2 (new)\n line 3\n";
    let patched = apply(repo, diff).unwrap();
    assert_eq!(
        patched.texts,
        vec![(
            repo.join("file.txt"),
            "line 1\nline 2 (new)\nline 3\n".to_string()
        )]
    );
}
