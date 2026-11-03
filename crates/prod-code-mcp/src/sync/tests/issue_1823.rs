/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! #1823: the executable bit crosses the sync watermark and survives a write-back.
#![cfg(unix)]

use crate::sync::cache::*;
use crate::sync::*;
use prod_code_protocol::FileDelta;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn git(root: &Path, args: &[&str]) -> bool {
    std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .status()
        .is_ok_and(|s| s.success())
}

fn git_repo(root: &Path) -> bool {
    if !git(root, &["init", "-q"]) {
        return false;
    }
    for (key, value) in [("user.name", "Test"), ("user.email", "test@example.com")] {
        assert!(git(root, &["config", key, value]));
    }
    true
}

fn chmod(path: &Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// `(path, is_executable)` of every planned upload.
fn uploads(plan: &SyncPlan) -> Vec<(String, bool)> {
    let mut sent: Vec<_> = plan
        .files
        .iter()
        .filter(|f| f.content.is_some())
        .map(|f| (f.relative_path.clone(), f.is_executable))
        .collect();
    sent.sort();
    sent
}

fn delta(path: &str, text: &str, is_executable: bool) -> FileDelta {
    FileDelta {
        relative_path: path.to_string(),
        content: Some(text.as_bytes().to_vec()),
        is_executable,
    }
}

const SCRIPT: &str = "#!/usr/bin/env python3\nprint('hi')\n";

/// The exact repro: a script synced, then `chmod +x` with the same bytes. The second sync
/// carried nothing, because the watermark (mtime, size, hash) did not change.
#[test]
fn issue_1823_mode_only_chmod_reaches_the_second_sync() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    clear_sync_cache(root);
    if !git_repo(root) {
        return;
    }
    std::fs::write(root.join("tool.py"), SCRIPT).unwrap();
    std::fs::write(root.join("lib.py"), "VALUE = 1\n").unwrap();
    chmod(&root.join("tool.py"), 0o644);
    chmod(&root.join("lib.py"), 0o644);
    assert!(git(root, &["add", "."]));
    assert!(git(root, &["commit", "-qm", "initial"]));
    let first = prepare_workspace_sync(root, None).unwrap();
    assert_eq!(
        uploads(&first),
        [("lib.py".into(), false), ("tool.py".into(), false)]
    );
    commit_workspace_sync(root, &first);
    assert!(prepare_workspace_sync(root, None).unwrap().files.is_empty());

    chmod(&root.join("tool.py"), 0o755);
    let second = prepare_workspace_sync(root, None).unwrap();
    assert_eq!(uploads(&second), [("tool.py".into(), true)], "{second:?}");
    let manifest = second.manifest();
    let stamp = |rel: &str| manifest.iter().find(|s| s.relative_path == rel).cloned();
    assert_eq!(stamp("tool.py").unwrap().is_executable, Some(true));
    assert_eq!(stamp("lib.py").unwrap().is_executable, Some(false));
    commit_workspace_sync(root, &second);
    assert!(prepare_workspace_sync(root, None).unwrap().files.is_empty());

    // And back: dropping the bit is a change too, and the plain file never gains one.
    chmod(&root.join("tool.py"), 0o644);
    let third = prepare_workspace_sync(root, None).unwrap();
    assert_eq!(uploads(&third), [("tool.py".into(), false)]);
    clear_sync_cache(root);
}

/// A watermark from before modes were tracked: the executable file is planned once, without
/// any content change, and a plain file is not sent again.
#[test]
fn issue_1823_legacy_watermark_resends_only_executable_files() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    clear_sync_cache(root);
    if !git_repo(root) {
        return;
    }
    std::fs::write(root.join("tool.py"), SCRIPT).unwrap();
    std::fs::write(root.join("lib.py"), "VALUE = 1\n").unwrap();
    chmod(&root.join("tool.py"), 0o755);
    assert!(git(root, &["add", "."]));
    assert!(git(root, &["commit", "-qm", "initial"]));
    let first = prepare_workspace_sync(root, None).unwrap();
    commit_workspace_sync(root, &first);

    let canonical = std::fs::canonicalize(root).unwrap();
    let mut legacy = load_sync_cache(&canonical);
    for entry in legacy.files.values_mut() {
        entry.executable = None;
    }
    save_sync_cache(&canonical, &legacy);
    let json = serde_json::to_string(&legacy).unwrap();
    assert!(!json.contains("executable"), "{json}");
    let legacy_manifest = SyncPlan {
        files: vec![],
        state: legacy,
        node: String::new(),
        initial: false,
    }
    .manifest();
    assert_eq!(legacy_manifest.len(), 2);
    assert!(
        legacy_manifest.iter().all(|s| s.is_executable.is_none()),
        "an unknown mode is not reported as 'not executable': {legacy_manifest:?}"
    );

    let upgraded = prepare_workspace_sync(root, None).unwrap();
    assert_eq!(uploads(&upgraded), [("tool.py".into(), true)]);
    commit_workspace_sync(root, &upgraded);
    assert!(prepare_workspace_sync(root, None).unwrap().files.is_empty());
    clear_sync_cache(root);
}

/// A formatter's write-back keeps the local 0755 and 0640 although the node's copy says "not
/// executable"; a plain file the node calls executable stays plain; new files take the node's
/// bit. The watermark records the node's bit, so the next sync repairs the node copy.
#[test]
fn issue_1823_writeback_keeps_local_modes_and_repairs_the_node_next_sync() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    clear_sync_cache(root);
    if !git_repo(root) {
        return;
    }
    std::fs::write(
        root.join("tool.py"),
        "#!/usr/bin/env python3\nprint( 'hi' )\n",
    )
    .unwrap();
    std::fs::write(root.join("private.py"), "X=1\n").unwrap();
    std::fs::write(root.join("plain.py"), "Y=2\n").unwrap();
    chmod(&root.join("tool.py"), 0o755);
    chmod(&root.join("private.py"), 0o640);
    chmod(&root.join("plain.py"), 0o644);
    assert!(git(root, &["add", "."]));
    assert!(git(root, &["commit", "-qm", "initial"]));
    let first = prepare_workspace_sync(root, None).unwrap();
    commit_workspace_sync(root, &first);

    let touched = apply_pulled_files_with(
        root,
        "",
        &[
            delta("tool.py", SCRIPT, false),
            delta("private.py", "X = 1\n", false),
            delta("plain.py", "Y = 2\n", true),
            delta("gen/new_tool.py", SCRIPT, true),
            delta("gen/new_data.py", "Z = 3\n", false),
        ],
        LocalMode::Preserve,
    )
    .unwrap();
    assert_eq!(touched.len(), 5);
    assert_eq!(
        std::fs::read_to_string(root.join("tool.py")).unwrap(),
        SCRIPT
    );
    assert_eq!(mode(&root.join("tool.py")), 0o755);
    assert_eq!(mode(&root.join("private.py")), 0o640);
    assert_eq!(mode(&root.join("plain.py")), 0o644);
    assert_ne!(mode(&root.join("gen/new_tool.py")) & 0o100, 0);
    assert_eq!(mode(&root.join("gen/new_data.py")) & 0o111, 0);

    // The node holds tool.py without the bit and plain.py with it: both are sent back with
    // the local bit. The files whose bit agrees are not.
    let next = prepare_workspace_sync(root, None).unwrap();
    assert_eq!(
        uploads(&next),
        [("plain.py".into(), false), ("tool.py".into(), true)],
        "{next:?}"
    );
    clear_sync_cache(root);
}

/// The atomic swap is confined: links out of the checkout, a parent that resolves elsewhere,
/// special files and `..` paths are refused and nothing outside is created or changed. A
/// contained link keeps being a link, and its target keeps its mode.
#[test]
fn issue_1823_writeback_refuses_unsafe_paths() {
    let temp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let root = temp.path();
    clear_sync_cache(root);
    let secret = outside.path().join("secret.py");
    std::fs::write(&secret, "outside\n").unwrap();
    chmod(&secret, 0o600);
    std::os::unix::fs::symlink(&secret, root.join("escape.py")).unwrap();
    std::os::unix::fs::symlink(outside.path(), root.join("out")).unwrap();
    std::fs::create_dir_all(root.join("dir.py")).unwrap();
    std::fs::write(root.join("real.py"), "old\n").unwrap();
    chmod(&root.join("real.py"), 0o750);
    std::os::unix::fs::symlink("real.py", root.join("alias.py")).unwrap();

    let touched = apply_pulled_files_with(
        root,
        "",
        &[
            delta("escape.py", "pwned\n", true),
            delta("out/new.py", "pwned\n", true),
            delta("out/deep/new.py", "pwned\n", true),
            delta("dir.py", "pwned\n", false),
            delta("../sibling.py", "pwned\n", false),
            delta("alias.py", "new\n", false),
        ],
        LocalMode::Preserve,
    )
    .unwrap();
    assert_eq!(touched, ["alias.py"]);
    assert_eq!(std::fs::read_to_string(&secret).unwrap(), "outside\n");
    assert_eq!(mode(&secret), 0o600);
    assert!(!outside.path().join("new.py").exists());
    assert!(!outside.path().join("deep").exists());
    assert!(root.join("dir.py").is_dir());
    assert!(!root.parent().unwrap().join("sibling.py").exists());
    assert!(
        std::fs::symlink_metadata(root.join("alias.py"))
            .unwrap()
            .is_symlink()
    );
    assert_eq!(
        std::fs::read_to_string(root.join("real.py")).unwrap(),
        "new\n"
    );
    assert_eq!(mode(&root.join("real.py")), 0o750);
    let leftovers: Vec<_> = std::fs::read_dir(root)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().contains(".prod-code-pull-"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    clear_sync_cache(root);
}
