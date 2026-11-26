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
fn coverage_w2_git_status_copy_rename_and_truncated_records() {
    for (input, expected) in [
        (
            &b"C100\0source.rs\0copy.rs\0D\0gone.rs\0"[..],
            vec![("copy.rs", false), ("gone.rs", true)],
        ),
        (
            &b"R100\0old.rs\0new.rs\0"[..],
            vec![("new.rs", false), ("old.rs", true)],
        ),
        (
            &b"M\0kept.rs\0C100\0source.rs\0"[..],
            vec![("kept.rs", false)],
        ),
        (&b"R100\0old.rs\0"[..], vec![]),
        (&b"M\0"[..], vec![]),
    ] {
        let mut paths = BTreeMap::new();
        parse_name_status(input, &mut paths);
        assert_eq!(
            paths,
            expected.into_iter().map(|(p, d)| (p.into(), d)).collect()
        );
    }
    for (input, expected) in [
        (
            &b"x\0C  copy.rs\0source.rs\0?? new file.rs\0"[..],
            vec![("copy.rs", false), ("new file.rs", false)],
        ),
        (
            &b" R new.rs\0old.rs\0 D gone.rs\0"[..],
            vec![("gone.rs", true), ("new.rs", false), ("old.rs", true)],
        ),
        (&b"R  new.rs\0"[..], vec![]),
        (&b"C  copy.rs\0"[..], vec![]),
    ] {
        let mut paths = BTreeMap::new();
        parse_porcelain_status(input, &mut paths);
        assert_eq!(
            paths,
            expected.into_iter().map(|(p, d)| (p.into(), d)).collect()
        );
    }
}

#[test]
fn coverage_w2_git_queries_report_non_checkout_and_outside_paths() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        git_head(dir.path()).unwrap_err().to_string(),
        "git rev-parse HEAD non-zero exit"
    );
    assert_eq!(
        git_output(dir.path(), ["status"]).unwrap_err().to_string(),
        "git command non-zero exit"
    );
    assert!(git_listed_paths(dir.path(), &["file.rs".into()]).is_empty());
    assert_eq!(git_listed_files(dir.path(), dir.path()), None);
    assert_eq!(
        git_listed_files(dir.path(), &dir.path().join("..").join("outside")),
        None
    );
}

#[cfg(unix)]
#[test]
fn tracked_directory_symlink_within_workspace_expands_to_member_files() {
    use std::os::unix::fs::symlink;

    let outer = tempfile::tempdir().unwrap();
    let outside = outer.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("secret.swift"), "let secret = 1\n").unwrap();

    let repo = outer.path().join("repo");
    let core_ui = repo.join("swift/Sources/TakoCoreUI");
    let app = repo.join("swift/Sources/TakoApp");
    std::fs::create_dir_all(&core_ui).unwrap();
    std::fs::create_dir_all(&app).unwrap();
    std::fs::write(
        core_ui.join("Theme.swift"),
        "public enum Theme { public static let size = 12 }\n",
    )
    .unwrap();
    std::fs::write(app.join("App.swift"), "let x = Theme.size\n").unwrap();
    symlink(Path::new("../TakoCoreUI"), app.join("_TakoCoreUI")).unwrap();
    symlink(&outside, app.join("_Outside")).unwrap();

    let run_git = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    };
    run_git(&["init", "-q"]);
    run_git(&["config", "user.email", "alex@prod.codes"]);
    run_git(&["config", "user.name", "Alex"]);
    run_git(&["add", "."]);
    run_git(&["commit", "-q", "-m", "initial"]);

    let listed = git_listed_files(&repo, &repo).unwrap();
    assert!(
        listed.contains(&"swift/Sources/TakoApp/_TakoCoreUI/Theme.swift".to_string()),
        "contained directory symlink must expand: {listed:?}"
    );
    assert!(
        !listed.iter().any(|p| p.contains("_Outside")),
        "escaping directory symlink must not expand: {listed:?}"
    );

    let scanned = crate::sync::scan::scan_workspace_files(&repo, None).unwrap();
    assert!(
        scanned.iter().any(
            |d| d.relative_path == "swift/Sources/TakoApp/_TakoCoreUI/Theme.swift"
                && d.content.is_some()
        ),
        "scan_workspace_files must materialize files under contained directory symlink"
    );

    let plan = crate::sync::plan::prepare_workspace_sync_for(&repo, "node-test", None).unwrap();
    assert!(
        plan.files.iter().any(|d| d.relative_path
            == "swift/Sources/TakoApp/_TakoCoreUI/Theme.swift"
            && d.content.is_some()),
        "prepare_workspace_sync_for must include files under contained directory symlink"
    );
}

/// #2096: an untracked directory holding another checkout (a worktree created inside this one)
/// is listed by git as `wt/`; it is not a file, so it is neither sent nor, once gone, deleted.
#[test]
fn a_nested_checkout_is_not_a_file_to_sync() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let git = |dir: &Path, args: &[&str]| {
        std::process::Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(dir)
            .status()
            .is_ok_and(|s| s.success())
    };
    if !git(&root, &["init", "-q"]) {
        return; // no git here
    }
    std::fs::write(root.join("lib.rs"), "fn main() {}\n").unwrap();
    assert!(git(&root, &["add", "."]) && git(&root, &["commit", "-qm", "base"]));
    let nested = root.join("wt");
    std::fs::create_dir(&nested).unwrap();
    assert!(git(&nested, &["init", "-q"]));
    std::fs::write(nested.join("main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(root.join("new.rs"), "fn f() {}\n").unwrap();

    let head = git_head(&root).unwrap();
    let (paths, dirty) = changed_paths(&root, Some(&head), &head).unwrap();
    assert!(paths.contains_key("new.rs"), "{paths:?}");
    assert!(!paths.keys().any(|p| p.starts_with("wt")), "{paths:?}");
    assert!(!dirty.iter().any(|p| p.starts_with("wt")), "{dirty:?}");
    let listed = git_listed_files(&root, &root).unwrap();
    assert!(!listed.iter().any(|p| p.ends_with('/')), "{listed:?}");
    let deltas = collect_git_dirty_files(&root, false).unwrap();
    assert!(
        !deltas.iter().any(|d| d.relative_path.starts_with("wt")),
        "{:?}",
        deltas.iter().map(|d| &d.relative_path).collect::<Vec<_>>()
    );

    std::fs::remove_dir_all(&nested).unwrap();
    let (paths, _) = changed_paths(&root, Some(&head), &head).unwrap();
    assert!(!paths.keys().any(|p| p.starts_with("wt")), "{paths:?}");
}
