/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::sync::entry::*;
use crate::sync::scan::*;
use crate::sync::*;
use prod_code_protocol::FileDelta;
use std::path::Path;

#[test]
fn a_vendored_library_is_synced_and_may_be_large() {
    assert!(is_synced_git_path(
        "internal/takovt/vendor/include/prod_vt_checkpoint.h"
    ));
    assert!(is_synced_git_path(
        "internal/takovt/vendor/lib/darwin-arm64/libtako_core.a"
    ));
    assert!(is_synced_git_path("vendor/github.com/pkg/errors/errors.go"));
    assert!(!is_synced_git_path("data/prices.csv"));
    assert!(!is_synced_git_path("dist/app.js"));
    assert_eq!(size_limit("vendor/lib/libtako_core.a"), MAX_LIBRARY_SIZE);
    assert_eq!(size_limit("lib/libfoo.dylib"), MAX_LIBRARY_SIZE);
    assert_eq!(size_limit("src/main.go"), MAX_FILE_SIZE);
    assert_eq!(size_limit("assets/video.mp4"), MAX_FILE_SIZE);
}

#[test]
#[allow(clippy::assertions_on_constants)]
fn json_config_size_limit_allows_tracked_build_metadata() {
    assert!(3_176_495 <= MAX_JSON_CONFIG_SIZE); // download-metadata.json in uv-python (#738)
    assert!(MAX_JSON_CONFIG_SIZE <= MAX_FILE_SIZE);
}

/// A sync is cut into messages that stay under the frame limit: files are packed in order up
/// to the budget, one larger than the budget travels alone, and an empty delta is still one
/// (empty) message (#313).
#[test]
fn a_sync_is_cut_into_messages_under_the_budget() {
    let file = |name: &str, len: usize| FileDelta {
        relative_path: name.to_string(),
        content: Some(vec![b'x'; len]),
        is_executable: false,
    };
    let deleted = FileDelta {
        relative_path: "gone.rs".to_string(),
        content: None,
        is_executable: false,
    };
    let names = |batches: &[Vec<FileDelta>]| -> Vec<Vec<String>> {
        batches
            .iter()
            .map(|b| b.iter().map(|f| f.relative_path.clone()).collect())
            .collect()
    };
    let batches = sync_batches(
        vec![
            file("a", 4),
            file("b", 4),
            deleted.clone(),
            file("c", 3),
            file("huge", 25),
            file("d", 1),
        ],
        10,
    );
    assert_eq!(
        names(&batches),
        vec![
            vec!["a", "b", "gone.rs"],
            vec!["c"],
            vec!["huge"],
            vec!["d"],
        ]
    );
    assert_eq!(sync_batches(Vec::new(), 10), vec![Vec::<FileDelta>::new()]);
    assert_eq!(
        names(&sync_batches(vec![deleted], 10)),
        vec![vec!["gone.rs"]]
    );
}

/// A proposed source file can be routed before either it or its parent exists.
#[test]

fn a_file_git_lists_is_synced_whatever_its_extension() {
    for synced in [
        "tests/fixtures/ref/case/snapshot.txt",
        "tests/fixtures/ref/case/input.recording",
        "tests/fixtures/capture.bin",
        ".github/workflows/ci.yml",
        "docs/notes.md",
        "src/main.rs",
        "crates/x/data/table.csv",
        "vendor/lib/x.c",
    ] {
        assert!(is_synced_git_path(synced), "{synced} should be synced");
    }
    for kept_out in [
        ".git/config",
        "target/debug/app",
        "web/node_modules/x/index.js",
        "data/prices.csv",
        "research/bench.jsonl",
        "fixtures/.DS_Store",
    ] {
        assert!(!is_synced_git_path(kept_out), "{kept_out} should stay out");
    }
}

fn git(root: &Path, args: &[&str]) -> bool {
    std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .status()
        .is_ok_and(|s| s.success())
}

/// A checkout with fixtures of every kind: all of them reach the gateway, a git-ignored file
/// does not, and neither does a data tree (#123).
#[test]
fn fixtures_of_any_kind_reach_the_gateway_and_ignored_files_do_not() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    if !git(&root, &["init", "-q"]) {
        return; // no git here
    }
    let files: &[(&str, &[u8])] = &[
        ("Cargo.toml", b"[package]\nname = \"f\"\n"),
        ("src/lib.rs", b"pub fn f() {}\n"),
        ("tests/fixtures/case/snapshot.txt", b"snapshot\n"),
        ("tests/fixtures/case/input.recording", b"rec"),
        ("tests/fixtures/capture.bin", &[0u8, 159, 146, 150]),
        (".github/workflows/ci.yml", b"on: push\n"),
        ("data/big.csv", b"a,b\n"),
        ("scratch.log", b"ignored\n"),
        (".gitignore", b"*.log\n"),
    ];
    for (rel, body) in files {
        std::fs::create_dir_all(root.join(rel).parent().unwrap()).unwrap();
        std::fs::write(root.join(rel), body).unwrap();
    }
    // Half committed, half untracked: both are git's to list.
    assert!(git(
        &root,
        &["add", "Cargo.toml", "src", "tests/fixtures/case"]
    ));
    assert!(git(
        &root,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "init"
        ]
    ));

    let mut paths: Vec<String> = scan_workspace_files(&root, None)
        .unwrap()
        .into_iter()
        .map(|d| d.relative_path)
        .collect();
    paths.sort();
    assert_eq!(
        paths,
        [
            ".github/workflows/ci.yml",
            ".gitignore",
            "Cargo.toml",
            "src/lib.rs",
            "tests/fixtures/capture.bin",
            "tests/fixtures/case/input.recording",
            "tests/fixtures/case/snapshot.txt",
        ]
    );
    let fixtures = scan_workspace_files(&root, Some(Path::new("tests/fixtures"))).unwrap();
    assert_eq!(
        fixtures.len(),
        3,
        "a path limits the scan to what is under it"
    );
    let bin = fixtures
        .iter()
        .find(|d| d.relative_path.ends_with("capture.bin"))
        .unwrap();
    assert_eq!(bin.content.as_deref(), Some(&[0u8, 159, 146, 150][..]));

    // The sync plan, first contact: the same files.
    clear_sync_cache(&root);
    let plan = prepare_workspace_sync(&root, None).unwrap();
    let mut planned: Vec<&str> = plan
        .files
        .iter()
        .map(|f| f.relative_path.as_str())
        .collect();
    planned.sort();
    assert!(
        planned.contains(&"tests/fixtures/capture.bin"),
        "{planned:?}"
    );
    assert!(planned.contains(&".github/workflows/ci.yml"), "{planned:?}");
    assert!(!planned.contains(&"scratch.log"), "{planned:?}");
    assert!(!planned.contains(&"data/big.csv"), "{planned:?}");
    clear_sync_cache(&root);
}

#[test]
#[cfg(unix)]
fn test_scan_workspace_files_rejects_symlinks_in_non_git_sync() {
    let ws = tempfile::tempdir().expect("tempdir");
    let ws_root = ws.path();
    std::fs::create_dir_all(ws_root.join("src")).expect("src dir");
    std::fs::write(ws_root.join("src/valid.rs"), "pub fn ok() {}\n").expect("write valid");

    // External secret file
    let external = tempfile::tempdir().expect("external dir");
    let secret_file = external.path().join("id_rsa");
    std::fs::write(&secret_file, "secret-private-key-bytes").expect("write secret");

    // Create symlink inside workspace pointing to external secret
    let leak_symlink = ws_root.join("src/leak.rs");
    std::os::unix::fs::symlink(&secret_file, &leak_symlink).expect("symlink");

    let deltas = scan_workspace_files(ws_root, None).expect("scan");
    assert!(deltas.iter().any(|d| d.relative_path == "src/valid.rs"));
    assert!(
        !deltas.iter().any(|d| d.relative_path == "src/leak.rs"),
        "symlink pointing outside workspace root must be rejected"
    );
    for delta in &deltas {
        if let Some(content) = &delta.content {
            assert_ne!(
                content.as_slice(),
                b"secret-private-key-bytes",
                "leaked external content must not be present in any file delta"
            );
        }
    }
}

#[test]
#[cfg(unix)]
fn test_scan_workspace_files_rejects_git_listed_symlinks_in_commitless_repo() {
    let ws = tempfile::tempdir().expect("tempdir");
    let ws_root = ws.path();

    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(ws_root)
        .args(["init", "-q"])
        .status()
        .expect("git init");
    assert!(status.success());

    std::fs::create_dir_all(ws_root.join("src")).expect("src dir");
    std::fs::write(ws_root.join("src/valid.rs"), "pub fn ok() {}\n").expect("write valid");

    let external = tempfile::tempdir().expect("external dir");
    let secret_file = external.path().join("id_rsa");
    std::fs::write(&secret_file, "secret-private-key-bytes").expect("write secret");

    let leak_symlink = ws_root.join("src/leak.rs");
    std::os::unix::fs::symlink(&secret_file, &leak_symlink).expect("symlink");

    let deltas = scan_workspace_files(ws_root, None).expect("scan");
    assert!(deltas.iter().any(|d| d.relative_path == "src/valid.rs"));
    assert!(
        !deltas.iter().any(|d| d.relative_path == "src/leak.rs"),
        "git-listed symlink pointing outside workspace root must be rejected"
    );
    for delta in &deltas {
        if let Some(content) = &delta.content {
            assert_ne!(
                content.as_slice(),
                b"secret-private-key-bytes",
                "leaked external content must not be present in any file delta"
            );
        }
    }
}
