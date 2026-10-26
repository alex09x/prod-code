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
use serde_json::json;
use std::time::Duration;

const DELETE_OLD: &str = "--- a/gone.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-old\n";
const MODIFY: &str = "--- a/kept.rs\n+++ b/kept.rs\n@@ -1 +1 @@\n-fn old() {}\n+fn new() {}\n";

const VALID_CARRIAGE_RETURNS: &[(&str, &str)] = &[
    (
        "old\r\n",
        "--- a/gone.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-old\r\n",
    ),
    (
        "old\r",
        "--- a/gone.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-old\r\n\\ No newline at end of file\n",
    ),
    (
        "\r",
        "--- a/gone.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-\r\n\\ No newline at end of file\n",
    ),
    (
        "one\r\ntwo\r",
        "--- a/gone.rs\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-one\r\n-two\r\n\\ No newline at end of file\n",
    ),
];

// Literal inputs are retained here; test execution never writes tracked evidence.
const INVALID: &[(&str, &str, &str)] = &[
    ("stale", "changed\n", DELETE_OLD),
    ("LF changed to CRLF", "old\r\n", DELETE_OLD),
    ("CRLF changed to LF", "old\n", VALID_CARRIAGE_RETURNS[0].1),
    ("trailing CR removed", "old", VALID_CARRIAGE_RETURNS[1].1),
    ("incomplete", "old\nremaining\n", DELETE_OLD),
    ("no hunks", "old\n", "--- a/gone.rs\n+++ /dev/null\n"),
    (
        "truncated",
        "old\n",
        "--- a/gone.rs\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-old\n",
    ),
    (
        "wrong count",
        "old\n",
        "--- a/gone.rs\n+++ /dev/null\n@@ -1,no +0,0 @@\n-old\n",
    ),
    (
        "wrong new range",
        "old\n",
        "--- a/gone.rs\n+++ /dev/null\n@@ -1 +1 @@\n-old\n",
    ),
    (
        "bad header",
        "old\n",
        "--- a/gone.rs\n+++ /dev/null\n@@ -1 +0,0\n-old\n",
    ),
    (
        "surplus",
        "old\n",
        "--- a/gone.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-old\n-extra\n",
    ),
    (
        "replacement",
        "old\n",
        "--- a/gone.rs\n+++ /dev/null\n@@ -1 +1 @@\n-old\n+replacement\n",
    ),
    (
        "wrong start",
        "old\n",
        "--- a/gone.rs\n+++ /dev/null\n@@ -9 +0,0 @@\n-old\n",
    ),
    ("newline changed", "old", DELETE_OLD),
    (
        "extra marker",
        "old\n",
        "--- a/gone.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-old\n\\ No newline at end of file\n",
    ),
    (
        "fake marker",
        "old",
        "--- a/gone.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-old\n\\ anything\n",
    ),
    (
        "empty hunk",
        "old\n",
        "--- a/gone.rs\n+++ /dev/null\n@@ -0,0 +0,0 @@\n",
    ),
];

#[test]
fn deletion_rejects_stale_malformed_and_incomplete_live_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gone.rs");
    let mut accepted = Vec::new();
    for &(name, current, diff) in INVALID {
        std::fs::write(&path, current).unwrap();
        if apply(dir.path(), diff).is_ok() {
            accepted.push(name);
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), current);
    }
    assert!(
        accepted.is_empty(),
        "invalid deletions accepted: {accepted:?}"
    );
}

#[test]
fn deletion_requires_a_live_file_even_with_a_git_preimage() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gone.rs");
    std::fs::write(&path, "old\n").unwrap();
    for args in [vec!["init"], vec!["add", "gone.rs"]] {
        let result = std::process::Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert!(result.status.success(), "{result:?}");
    }
    assert_eq!(
        git_read_blob(dir.path(), "gone.rs").as_deref(),
        Some("old\n")
    );
    std::fs::write(&path, "changed\n").unwrap();
    let stale_refused = apply(dir.path(), DELETE_OLD).is_err();
    std::fs::remove_file(&path).unwrap();
    let missing_refused = apply(dir.path(), DELETE_OLD).is_err();
    assert!(
        stale_refused && missing_refused,
        "stale refused={stale_refused}, missing refused={missing_refused}"
    );
}

#[test]
fn deletion_preserves_valid_full_file_and_empty_file_forms() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gone.rs");
    for (current, diff) in [
        ("old\n", DELETE_OLD),
        (
            "old",
            "--- a/gone.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-old\n\\ No newline at end of file\n",
        ),
        (
            "one\ntwo\n",
            "--- a/gone.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-one\n@@ -2 +0,0 @@\n-two\n",
        ),
        (
            "-- header-looking\n+++ literal\n",
            "--- a/gone.rs\n+++ /dev/null\n@@ -1,2 +0,0 @@\n--- header-looking\n-+++ literal\n",
        ),
        ("", "--- a/gone.rs\n+++ /dev/null\n@@ -0,0 +0,0 @@\n"),
    ]
    .into_iter()
    .chain(VALID_CARRIAGE_RETURNS.iter().copied())
    {
        std::fs::write(&path, current).unwrap();
        let patched = apply(dir.path(), diff).unwrap();
        assert_eq!(patched.deleted, vec![path.clone()]);
        assert!(patched.texts.is_empty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), current);
    }
}

#[tokio::test]
async fn deletion_invalidates_the_whole_proposal_before_analyzer_or_writes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut accepted = Vec::new();
    for &(name, current, deletion) in INVALID {
        for diff in [
            deletion.to_string(),
            format!("{MODIFY}{deletion}"),
            format!("{deletion}{MODIFY}"),
        ] {
            for compile in [false, true] {
                std::fs::write(root.join("gone.rs"), current).unwrap();
                std::fs::write(root.join("kept.rs"), "fn old() {}\n").unwrap();
                let args = json!({"diff": diff, "compile": compile});
                let outcome = tokio::time::timeout(
                    Duration::from_millis(200),
                    crate::tools::handle_validate_edits(
                        listener.local_addr().unwrap(),
                        root,
                        &args,
                    ),
                )
                .await;
                // Only a synchronous proposal parse failure can satisfy this assertion.
                if !matches!(outcome, Ok(Err(_))) {
                    accepted.push((name, compile));
                }
                assert_eq!(
                    std::fs::read_to_string(root.join("gone.rs")).unwrap(),
                    current
                );
                assert_eq!(
                    std::fs::read_to_string(root.join("kept.rs")).unwrap(),
                    "fn old() {}\n"
                );
            }
        }
    }
    assert!(
        accepted.is_empty(),
        "invalid proposals reached validation: {accepted:?}"
    );
    assert!(
        matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock),
        "analyzer/compiler connection attempted"
    );
}

#[tokio::test]
async fn deletion_of_a_missing_file_refuses_mixed_proposals_before_analyzer() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    std::fs::write(root.join("kept.rs"), "fn old() {}\n").unwrap();
    for diff in [
        DELETE_OLD.to_string(),
        format!("{MODIFY}{DELETE_OLD}"),
        format!("{DELETE_OLD}{MODIFY}"),
    ] {
        let args = json!({"diff": diff, "compile": true});
        let error = tokio::time::timeout(
            Duration::from_millis(200),
            crate::tools::handle_validate_edits(listener.local_addr().unwrap(), root, &args),
        )
        .await
        .expect("proposal must fail before analyzer work")
        .unwrap_err();
        assert!(error.to_string().contains("cannot be read"), "{error:#}");
        assert!(!root.join("gone.rs").exists());
        assert_eq!(
            std::fs::read_to_string(root.join("kept.rs")).unwrap(),
            "fn old() {}\n"
        );
    }
    assert!(matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock));
}

#[tokio::test]
async fn deletion_valid_full_and_zero_line_proposals_remain_supported() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    for (current, diff) in [
        ("old\n", DELETE_OLD),
        (
            "old",
            "--- a/gone.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-old\n\\ No newline at end of file\n",
        ),
        ("", "--- a/gone.rs\n+++ /dev/null\n@@ -0,0 +0,0 @@\n"),
    ]
    .into_iter()
    .chain(VALID_CARRIAGE_RETURNS.iter().copied())
    {
        std::fs::write(root.join("gone.rs"), current).unwrap();
        let result = crate::tools::handle_validate_edits(
            listener.local_addr().unwrap(),
            root,
            &json!({"diff": diff}),
        )
        .await
        .unwrap();
        assert!(!result.is_error, "{result:?}");
        assert_eq!(
            std::fs::read_to_string(root.join("gone.rs")).unwrap(),
            current
        );
    }
    assert!(matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock));
}

#[test]
fn deletion_keeps_header_looking_modifications_and_additions_in_mixed_patches() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("gone.rs"), "old\n").unwrap();
    std::fs::write(root.join("kept.rs"), "-- old\n++ literal\n").unwrap();
    let modification =
        "--- a/kept.rs\n+++ b/kept.rs\n@@ -1,2 +1,2 @@\n--- old\n+++ new\n ++ literal\n";
    let addition =
        "--- /dev/null\n+++ b/new.rs\n@@ -0,0 +1 @@\n+++ new\n\\ No newline at end of file\n";
    for diff in [
        format!("{DELETE_OLD}{modification}{addition}"),
        format!("{addition}{modification}{DELETE_OLD}"),
    ] {
        let patched = apply(root, &diff).unwrap();
        assert_eq!(patched.deleted, vec![root.join("gone.rs")]);
        assert_eq!(patched.texts.len(), 2);
        assert!(
            patched
                .texts
                .contains(&(root.join("kept.rs"), "++ new\n++ literal\n".into()))
        );
        assert!(
            patched
                .texts
                .contains(&(root.join("new.rs"), "++ new".into()))
        );
    }
    assert_eq!(
        std::fs::read_to_string(root.join("kept.rs")).unwrap(),
        "-- old\n++ literal\n"
    );
    assert!(!root.join("new.rs").exists());
}
