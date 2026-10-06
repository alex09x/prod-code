/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::sync::cache::*;
use crate::sync::*;
use prod_code_protocol::{FileDelta, content_hash};
use std::path::Path;
fn git(root: &Path, args: &[&str]) -> bool {
    std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .status()
        .is_ok_and(|s| s.success())
}

#[test]
fn stale_paths_from_a_handshake_are_in_the_next_sync() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    clear_sync_cache(root);
    if !std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(root)
        .status()
        .unwrap()
        .success()
    {
        return;
    }
    for (key, value) in [("user.name", "Test"), ("user.email", "test@example.com")] {
        assert!(git(root, &["config", key, value]));
    }
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn a() {}").unwrap();
    std::fs::write(root.join("src/other.rs"), "pub fn b() {}").unwrap();
    std::fs::write(root.join(".gitignore"), "secret.env\n").unwrap();
    std::fs::write(root.join("secret.env"), "TOKEN=1").unwrap();
    assert!(git(root, &["add", "."]));
    assert!(git(root, &["commit", "-qm", "initial"]));
    let first = prepare_workspace_sync(root, None).unwrap();
    commit_workspace_sync(root, &first);
    assert!(prepare_workspace_sync(root, None).unwrap().files.is_empty());

    let handshake = prod_code_protocol::HandshakeResponse {
        protocol_version: prod_code_protocol::PROTOCOL_VERSION,
        server_pid: 1,
        session_id: 1,
        server_workspace_root: "/srv/ws".to_string(),
        detected_engine: "rust".to_string(),
        stale_paths: [
            "src/lib.rs",
            "src/generated.rs",
            "secret.env",
            "../outside.rs",
        ]
        .map(str::to_string)
        .to_vec(),
        engine_age_ms: None,
        index_gated: false,
        capabilities: None,
    };
    resend_lost_files(root, "", &handshake.stale_paths);

    let plan = prepare_workspace_sync(root, None).unwrap();
    let mut sent: Vec<(&str, Option<&[u8]>)> = plan
        .files
        .iter()
        .map(|f| (f.relative_path.as_str(), f.content.as_deref()))
        .collect();
    sent.sort();
    assert_eq!(
        sent,
        [
            ("secret.env", None),
            ("src/generated.rs", None),
            ("src/lib.rs", Some(b"pub fn a() {}".as_slice())),
        ]
    );
    commit_workspace_sync(root, &plan);
    let next = prepare_workspace_sync(root, None).unwrap();
    assert!(next.files.is_empty(), "sent once: {next:?}");
    clear_sync_cache(root);
}

#[test]
fn test_apply_pulled_files_writes_and_records_watermark() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    clear_sync_cache(root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/old.rs"), "x").unwrap();
    let files = vec![
        FileDelta {
            relative_path: "src/fmt.rs".to_string(),
            content: Some(b"fn f() {}\n".to_vec()),
            is_executable: false,
        },
        FileDelta {
            relative_path: "src/old.rs".to_string(),
            content: None,
            is_executable: false,
        },
        FileDelta {
            relative_path: "../escape.rs".to_string(),
            content: Some(b"no".to_vec()),
            is_executable: false,
        },
    ];
    let touched = apply_pulled_files(root, &files).unwrap();
    assert_eq!(
        touched,
        vec!["src/fmt.rs".to_string(), "src/old.rs".to_string()]
    );
    assert_eq!(
        std::fs::read_to_string(root.join("src/fmt.rs")).unwrap(),
        "fn f() {}\n"
    );
    assert!(!root.join("src/old.rs").exists());
    assert!(!root.join("../escape.rs").exists());
    let state = load_sync_cache(&std::fs::canonicalize(root).unwrap());
    assert_eq!(
        state.files.get("src/fmt.rs").map(|e| e.hash),
        Some(content_hash(b"fn f() {}\n"))
    );
    clear_sync_cache(root);
}

#[test]

fn test_workspace_identity_isolates_worktrees() {
    let temp = tempfile::tempdir().unwrap();
    let origin = temp.path().join("my-repo");
    std::fs::create_dir_all(&origin).unwrap();
    let init = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(&origin)
        .status()
        .unwrap();
    if !init.success() {
        return;
    }
    for (key, value) in [("user.name", "Test"), ("user.email", "test@example.com")] {
        assert!(
            std::process::Command::new("git")
                .args(["config", key, value])
                .current_dir(&origin)
                .status()
                .unwrap()
                .success()
        );
    }
    std::fs::write(origin.join("a.rs"), "pub fn a() {}").unwrap();
    for args in [&["add", "a.rs"][..], &["commit", "-qm", "initial"][..]] {
        assert!(
            std::process::Command::new("git")
                .args(args)
                .current_dir(&origin)
                .status()
                .unwrap()
                .success()
        );
    }
    let wt_a = temp.path().join("worktrees/task-1/attempt-0");
    let wt_b = temp.path().join("worktrees/task-2/attempt-0");
    for (wt, branch) in [(&wt_a, "wt-a"), (&wt_b, "wt-b")] {
        std::fs::create_dir_all(wt.parent().unwrap()).unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["worktree", "add", "-q", "-b", branch, wt.to_str().unwrap()])
                .current_dir(&origin)
                .status()
                .unwrap()
                .success()
        );
    }

    let main = workspace_identity(&origin);
    assert_eq!(main.name, "my-repo");
    assert_eq!(main.base, None);

    let a = workspace_identity(&wt_a);
    let b = workspace_identity(&wt_b);
    assert!(a.name.starts_with("my-repo--wt-"), "{}", a.name);
    assert!(b.name.starts_with("my-repo--wt-"), "{}", b.name);
    assert_ne!(a.name, b.name);
    assert_eq!(a.base.as_deref(), Some("my-repo"));
    assert_eq!(workspace_identity(&wt_a), a);
}

#[test]
fn test_partial_sync_does_not_advance_workspace_base() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    clear_sync_cache(root);

    let init = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(root)
        .status()
        .unwrap();
    if !init.success() {
        return;
    }
    for (key, value) in [("user.name", "Test"), ("user.email", "test@example.com")] {
        let configured = std::process::Command::new("git")
            .args(["config", key, value])
            .current_dir(root)
            .status()
            .unwrap();
        assert!(configured.success());
    }

    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.rs"), "pub const A: u8 = 1;").unwrap();
    std::fs::write(root.join("src/b.rs"), "pub const B: u8 = 1;").unwrap();
    for args in [&["add", "src"][..], &["commit", "-qm", "initial"][..]] {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .status()
            .unwrap();
        assert!(status.success());
    }
    let initial = prepare_workspace_sync(root, None).unwrap();
    commit_workspace_sync(root, &initial);
    let initial_base = load_sync_cache(root).base_commit_sha;

    std::fs::write(root.join("src/a.rs"), "pub const A: u8 = 2;").unwrap();
    std::fs::write(root.join("src/b.rs"), "pub const B: u8 = 2;").unwrap();
    for args in [&["add", "src"][..], &["commit", "-qm", "both changed"][..]] {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .status()
            .unwrap();
        assert!(status.success());
    }

    let partial = prepare_workspace_sync(root, Some(Path::new("src/a.rs"))).unwrap();
    assert_eq!(partial.files.len(), 1);
    commit_workspace_sync(root, &partial);
    assert_eq!(load_sync_cache(root).base_commit_sha, initial_base);

    let remaining = prepare_workspace_sync(root, None).unwrap();
    assert_eq!(remaining.files.len(), 1);
    assert_eq!(remaining.files[0].relative_path, "src/b.rs");
    clear_sync_cache(root);
}
