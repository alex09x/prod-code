/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::UploadExclusionReason::*;
use super::*;
use crate::sync::scan::scan_workspace_files;

fn write(root: &Path, rel: &str, bytes: &[u8]) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn git(root: &Path, args: &[&str]) -> bool {
    std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .status()
        .is_ok_and(|s| s.success())
}

fn reasons(excluded: &[UploadExclusion]) -> Vec<(&str, UploadExclusionReason)> {
    let mut out: Vec<_> = excluded
        .iter()
        .map(|e| (e.path.as_str(), e.reason))
        .collect();
    out.sort_by(|a, b| a.0.cmp(b.0));
    out
}

fn oversized() -> Vec<u8> {
    vec![b'/'; MAX_FILE_SIZE as usize + 1]
}

/// A Git checkout: git-ignored, output-directory and oversized files are named with their
/// reason; fixtures of any extension and deleted paths are not.
#[test]
fn issue_1813_git_checkout_names_ignored_output_and_oversized_files() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    if !git(&root, &["init", "-q"]) {
        return; // no git here
    }
    write(&root, ".gitignore", b"ignored/\n");
    write(&root, "src/lib.rs", b"pub fn f() {}\n");
    assert!(git(&root, &["add", "."]));
    assert!(git(
        &root,
        &[
            "-c",
            "user.name=T",
            "-c",
            "user.email=t@e",
            "commit",
            "-qm",
            "base"
        ]
    ));
    write(&root, "results/out.bin", &[0, 1, 2]);
    write(&root, "ignored/gen.rs", b"fn g() {}\n");
    write(&root, "src/big.rs", &oversized());
    write(&root, "fixture/out.bin", &[0, 159, 146]);
    write(&root, "tests/fixtures/meta.json", b"{}\n");
    let pulled: Vec<String> = [
        "results/out.bin",
        "ignored/gen.rs",
        "src/big.rs",
        "fixture/out.bin",
        "tests/fixtures/meta.json",
        "deleted.rs",
        "../outside.rs",
    ]
    .map(String::from)
    .to_vec();
    let excluded = classify_upload_exclusions(&root, &pulled);
    assert_eq!(
        reasons(&excluded),
        [
            ("ignored/gen.rs", GitIgnored),
            ("results/out.bin", OutputOrDataDirectory),
            ("src/big.rs", OverSizeLimit),
        ]
    );
    let warning = upload_exclusion_warning(&excluded).unwrap();
    assert!(warning.starts_with("upload warning: 3 written-back file(s)"));
    assert!(warning.contains("results/out.bin (output/data directory)"));
    assert!(warning.contains("ignored/gen.rs (git-ignored)"));
    assert!(warning.contains("src/big.rs (over size limit)"));
    assert!(warning.contains("[sync] include"));
    assert!(warning.contains("tests/fixtures/ path in a Git checkout"));
    assert!(warning.contains("--pin-node"));
    let json = serde_json::to_value(&excluded).unwrap();
    assert!(
        json.as_array()
            .unwrap()
            .iter()
            .any(|e| e["path"] == "results/out.bin" && e["reason"] == "output_or_data_directory")
    );
    let scanned: Vec<String> = scan_workspace_files(&root, None)
        .unwrap()
        .into_iter()
        .map(|d| d.relative_path)
        .collect();
    for path in &pulled[..5] {
        let out = excluded.iter().any(|e| &e.path == path);
        assert_eq!(scanned.contains(path), !out, "{path}");
    }
}

/// A directory without Git: watch-ignore, root `.gitignore`, hidden, extension, output
/// directory and size rules of the walk, each with its own reason, agreeing with the walk.
/// Root `.gitignore` without `.git` prunes directories only; a file the walk still uploads is
/// not named.
#[test]
fn issue_1813_non_git_walk_names_watch_ignored_hidden_and_oversized_files() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    write(
        &root,
        "prod-code.toml",
        b"[watch]\nignore = [\"generated/\"]\n",
    );
    write(&root, ".gitignore", b"skipdir/\nskip.rs\n");
    write(&root, "src/main.rs", b"fn main() {}\n");
    write(&root, "generated/out.rs", b"fn g() {}\n");
    write(&root, "skipdir/x.rs", b"fn s() {}\n");
    write(&root, "skip.rs", b"fn s() {}\n");
    write(&root, "results/out.bin", &[0, 1, 2]);
    write(&root, "notes.bin", &[0, 1, 2]);
    write(&root, ".hidden/x.rs", b"fn x() {}\n");
    write(&root, "src/big.rs", &oversized());
    let pulled: Vec<String> = [
        "src/main.rs",
        "generated/out.rs",
        "skipdir/x.rs",
        "skip.rs",
        "results/out.bin",
        "notes.bin",
        ".hidden/x.rs",
        "src/big.rs",
    ]
    .map(String::from)
    .to_vec();
    let excluded = classify_upload_exclusions(&root, &pulled);
    assert_eq!(
        reasons(&excluded),
        [
            (".hidden/x.rs", HiddenOrExtension),
            ("generated/out.rs", WatchIgnored),
            ("notes.bin", HiddenOrExtension),
            ("results/out.bin", OutputOrDataDirectory),
            ("skipdir/x.rs", GitIgnored),
            ("src/big.rs", OverSizeLimit),
        ]
    );
    let scanned: Vec<String> = scan_workspace_files(&root, None)
        .unwrap()
        .into_iter()
        .map(|d| d.relative_path)
        .collect();
    for path in &pulled {
        let out = excluded.iter().any(|e| &e.path == path);
        assert_eq!(scanned.contains(path), !out, "{path}");
    }
    let warning = upload_exclusion_warning(&excluded).unwrap();
    assert!(warning.contains("generated/out.rs (watch-ignored)"));
    assert!(warning.ends_with("(--pin-node)") && warning.contains(", and 1 more."));
    assert_eq!(upload_exclusion_warning(&[]), None);
}

/// Root and nested `.ignore` rules (including `!` negations) and non-Git `fixtures/*.csv`
/// match `scan_workspace_files` exactly, and `[sync] include` opts non-Git CSV/data in.
#[test]
fn issue_1813_non_git_walk_respects_root_and_nested_dot_ignore_and_csv_fixtures() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    write(&root, ".ignore", b"*.gen.rs\nsecret.json\n");
    write(&root, "sub/.ignore", b"*\n!keep.rs\n");
    write(&root, "src/lib.rs", b"pub fn ok() {}\n");
    write(&root, "src/out.gen.rs", b"pub fn gen_fn() {}\n");
    write(&root, "secret.json", b"{\"k\":1}\n");
    write(&root, "sub/drop.rs", b"pub fn drop_fn() {}\n");
    write(&root, "sub/keep.rs", b"pub fn keep_fn() {}\n");
    write(&root, "fixtures/sample.csv", b"a,b\n1,2\n");

    let pulled: Vec<String> = [
        "src/lib.rs",
        "src/out.gen.rs",
        "secret.json",
        "sub/drop.rs",
        "sub/keep.rs",
        "fixtures/sample.csv",
    ]
    .map(String::from)
    .to_vec();
    let excluded = classify_upload_exclusions(&root, &pulled);
    assert_eq!(
        reasons(&excluded),
        [
            ("fixtures/sample.csv", HiddenOrExtension),
            ("secret.json", GitIgnored),
            ("src/out.gen.rs", GitIgnored),
            ("sub/drop.rs", GitIgnored),
        ]
    );
    let scanned: Vec<String> = scan_workspace_files(&root, None)
        .unwrap()
        .into_iter()
        .map(|d| d.relative_path)
        .collect();
    for path in &pulled {
        let out = excluded.iter().any(|e| &e.path == path);
        assert_eq!(scanned.contains(path), !out, "{path}");
    }

    // Opting fixtures and secret.json into `[sync] include` uploads `fixtures/sample.csv`,
    // while `.ignore` still blocks `secret.json`.
    write(
        &root,
        "prod-code.toml",
        b"[sync]\ninclude = [\"fixtures/**\", \"secret.json\"]\n",
    );
    let excluded_after = classify_upload_exclusions(&root, &pulled);
    assert_eq!(
        reasons(&excluded_after),
        [
            ("secret.json", GitIgnored),
            ("src/out.gen.rs", GitIgnored),
            ("sub/drop.rs", GitIgnored),
        ]
    );
    let scanned_after: Vec<String> = scan_workspace_files(&root, None)
        .unwrap()
        .into_iter()
        .map(|d| d.relative_path)
        .collect();
    for path in &pulled {
        let out = excluded_after.iter().any(|e| &e.path == path);
        assert_eq!(scanned_after.contains(path), !out, "{path}");
    }
}

/// `git_listed_paths` checks only symlink ancestors of the supplied paths, never walking
/// sibling directories under a tracked directory symlink.
#[cfg(unix)]
#[test]
fn issue_1813_git_listed_paths_checks_tracked_dir_symlink_without_walking_siblings() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let outer = tempfile::tempdir().unwrap();
    let outside = outer.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("secret.rs"), "pub fn s() {}\n").unwrap();

    let repo = outer.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let root = std::fs::canonicalize(&repo).unwrap();
    if !git(&root, &["init", "-q"]) {
        return;
    }
    write(&root, ".gitignore", b"*.log\n");
    write(&root, "real_pkg/sub/mod.rs", b"pub fn m() {}\n");
    write(&root, "real_pkg/target/out.rs", b"pub fn t() {}\n");
    write(&root, "real_pkg/sealed/hidden.rs", b"pub fn h() {}\n");
    symlink(&outside, root.join("real_pkg/escape")).unwrap();
    symlink("real_pkg", root.join("linked_pkg")).unwrap();
    assert!(git(
        &root,
        &["add", ".gitignore", "real_pkg/sub/mod.rs", "linked_pkg"]
    ));
    assert!(git(
        &root,
        &[
            "-c",
            "user.name=T",
            "-c",
            "user.email=t@e",
            "commit",
            "-qm",
            "base"
        ]
    ));
    write(&root, "ignored.log", b"noise\n");

    let sealed = root.join("real_pkg/sealed");
    std::fs::set_permissions(&sealed, std::fs::Permissions::from_mode(0o000)).unwrap();

    let query: Vec<String> = [
        "linked_pkg/sub/mod.rs",
        "linked_pkg/target/out.rs",
        "linked_pkg/escape/secret.rs",
        "ignored.log",
    ]
    .map(String::from)
    .to_vec();
    let listed = git_listed_paths(&root, &query);
    std::fs::set_permissions(&sealed, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(listed, HashSet::from(["linked_pkg/sub/mod.rs".to_string()]));
}

#[test]
fn issue_1836_sync_include_rejects_escape_hidden_and_baseline_paths() {
    let temp = tempfile::tempdir().unwrap();
    let patterns: Vec<String> = [
        "../outside.json",
        "/etc/passwd",
        "target/**",
        "node_modules/**",
        ".env",
        "site/**",
        "./site/data/*.json",
    ]
    .map(String::from)
    .to_vec();
    let inc = SyncInclude::from_patterns(temp.path(), &patterns);
    assert!(inc.is_included("site/data/benchmarks-v0.3.17.json"));
    assert!(inc.may_contain_included(Path::new("site")));
    assert!(inc.may_contain_included(Path::new("site/data")));
    assert!(!inc.is_included("target/debug/out.json"));
    assert!(!inc.is_included("node_modules/pkg/package.json"));
    assert!(!inc.is_included(".env"));
    assert!(!inc.is_included("site/.env"));
    assert!(!inc.is_included("site/target/out.json"));
    assert!(!inc.may_contain_included(Path::new("target")));
    assert!(!inc.may_contain_included(Path::new("results")));
}

#[test]
fn issue_1836_non_git_sync_include_uploads_and_deletes_on_removal() {
    use crate::sync::{clear_sync_cache_for, commit_workspace_sync, prepare_workspace_sync_for};
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let node = "node-nongit.test:9400";
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/main.py"), "print('hi')\n").unwrap();
    std::fs::create_dir_all(root.join("site/data")).unwrap();
    std::fs::write(root.join("site/data/benchmarks.json"), b"{\"ok\":true}\n").unwrap();
    std::fs::create_dir_all(root.join("fixtures")).unwrap();
    std::fs::write(root.join("fixtures/rows.csv"), b"a,b\n1,2\n").unwrap();

    let plan1 = prepare_workspace_sync_for(&root, node, None).unwrap();
    assert!(
        plan1
            .omitted
            .iter()
            .any(|e| e.path == "site/data/"
                && e.reason == UploadExclusionReason::OutputOrDataDirectory),
        "{:?}",
        plan1.omitted
    );
    commit_workspace_sync(&root, &plan1);

    std::fs::write(
        root.join("prod-code.toml"),
        "[sync]\ninclude = [\"site/data/**\", \"fixtures/*.csv\"]\n",
    )
    .unwrap();
    let plan2 = prepare_workspace_sync_for(&root, node, None).unwrap();
    assert!(plan2.omitted.is_empty(), "{:?}", plan2.omitted);
    for expected in ["site/data/benchmarks.json", "fixtures/rows.csv"] {
        assert!(
            plan2
                .files
                .iter()
                .any(|f| f.relative_path == expected && f.content.is_some()),
            "missing {expected} in {:?}",
            plan2.files
        );
    }
    commit_workspace_sync(&root, &plan2);

    std::fs::remove_file(root.join("prod-code.toml")).unwrap();
    let plan3 = prepare_workspace_sync_for(&root, node, None).unwrap();
    for removed in ["site/data/benchmarks.json", "fixtures/rows.csv"] {
        assert!(
            plan3
                .files
                .iter()
                .any(|f| f.relative_path == removed && f.content.is_none()),
            "missing deletion for {removed} in {:?}",
            plan3.files
        );
    }
    clear_sync_cache_for(&root, node);
}
