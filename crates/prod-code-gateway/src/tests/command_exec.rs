/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::*;
use futures_util::StreamExt;
use prod_code_protocol::{ExecRequest, WireMessage};
use std::path::Path;
use tokio::net::TcpStream;
use tokio_util::codec::Framed;

/// A running command is in the status for as long as its handler runs, and gone however the
/// handler returns (#273).
#[test]
fn a_running_command_is_listed_until_its_handler_returns() {
    let workspace = Path::new("/srv/workspaces/shop--wt-status-test");
    let mine = |list: &[prod_code_protocol::RunningCommand]| {
        list.iter()
            .filter(|c| c.workspace == "shop--wt-status-test")
            .count()
    };
    let entry = RunningEntry::start(workspace, &["cargo".to_string(), "test".to_string()]);
    let listed = running_commands();
    assert_eq!(mine(&listed), 1);
    let command = listed
        .iter()
        .find(|c| c.workspace == "shop--wt-status-test")
        .unwrap();
    assert_eq!(command.command, "cargo test");
    drop(entry);
    assert_eq!(mine(&running_commands()), 0);
}

#[test]
fn a_compiler_cache_is_shared_across_worktrees_when_the_node_has_ccache() {
    let workspace = Path::new("/srv/workspaces/shop--wt-1a2b");
    assert!(compiler_cache_env(workspace, false).is_empty());
    let env = compiler_cache_env(workspace, true);
    let get = |k: &str| {
        env.iter()
            .find(|(key, _)| key == k)
            .map(|(_, v)| v.as_str())
    };
    assert_eq!(get("CCACHE_BASEDIR"), Some("/srv/workspaces/shop--wt-1a2b"));
    assert_eq!(get("CCACHE_NOHASHDIR"), Some("1"));
    assert_eq!(get("CCACHE_SLOPPINESS"), Some("pch_defines,time_macros"));
    assert_eq!(get("CCACHE_PCH_EXTSUM"), Some("1"));
    assert_eq!(get("CMAKE_C_COMPILER_LAUNCHER"), Some("ccache"));
    assert_eq!(get("CMAKE_CXX_COMPILER_LAUNCHER"), Some("ccache"));
    assert!(on_path("sh"), "sh is on PATH on every node");
    assert!(!on_path("no-such-program-on-any-node"));
}

/// A directory renamed or deleted locally leaves nothing behind on the copy (#124): the files
/// the manifest no longer lists go, and so do the directories that held only them, while a
/// directory that still holds a file, the workspace root and the per-node caches stay.
#[test]
fn a_directory_gone_locally_is_gone_from_the_copy() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    for (rel, body) in [
        ("Sources/OldApp/main.swift", "old"),
        ("Sources/Unused/deep/x.swift", "unused"),
        ("Sources/NewApp/main.swift", "new"),
        ("Package.swift", "pkg"),
        (".build/debug/cache.o", "cache"),
    ] {
        std::fs::create_dir_all(root.join(rel).parent().unwrap()).unwrap();
        std::fs::write(root.join(rel), body).unwrap();
    }
    std::fs::create_dir_all(root.join("Sources/Empty/deeper")).unwrap();
    let stamp = |rel: &str, body: &str| FileStamp {
        relative_path: rel.to_string(),
        size: body.len() as u64,
        hash: content_hash(body.as_bytes()),
    };
    let manifest = [
        stamp("Sources/NewApp/main.swift", "new"),
        stamp("Package.swift", "pkg"),
    ];
    let (missing, mut deleted) = reconcile_manifest(root, &manifest);
    deleted.sort();
    assert!(missing.is_empty(), "{missing:?}");
    assert_eq!(
        deleted,
        ["Sources/OldApp/main.swift", "Sources/Unused/deep/x.swift"]
    );
    for gone in ["Sources/OldApp", "Sources/Unused", "Sources/Empty"] {
        assert!(!root.join(gone).exists(), "{gone} is still on the copy");
    }
    assert!(root.join("Sources/NewApp/main.swift").is_file());
    assert!(
        root.join(".build/debug/cache.o").is_file(),
        "a node cache is not touched"
    );

    // A single deletion climbs only as far as the directories it empties.
    std::fs::create_dir_all(root.join("a/b/c")).unwrap();
    std::fs::write(root.join("a/keep.rs"), "k").unwrap();
    std::fs::write(root.join("a/b/c/gone.rs"), "g").unwrap();
    std::fs::remove_file(root.join("a/b/c/gone.rs")).unwrap();
    let emptied = root.join("a/b/c");
    prune_empty_parents(root, Some(&emptied));
    assert!(!root.join("a/b").exists());
    assert!(root.join("a/keep.rs").is_file());
    prune_empty_parents(root, Some(root));
    assert!(root.exists(), "the workspace root itself is never removed");
}

#[test]
fn test_changed_since_reports_new_changed_and_deleted() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("target/debug")).unwrap();
    std::fs::write(root.join("src/a.rs"), "a").unwrap();
    std::fs::write(root.join("src/gone.rs"), "g").unwrap();
    std::fs::write(root.join("Cargo.lock"), "l1").unwrap();
    let before = snapshot_tree(root);

    std::fs::write(root.join("src/a.rs"), "a formatted").unwrap();
    std::fs::write(root.join("src/new.rs"), "n").unwrap();
    std::fs::remove_file(root.join("src/gone.rs")).unwrap();
    std::fs::write(root.join("target/debug/junk.o"), "x").unwrap();

    let changed = changed_since(root, &before.stamps);
    let names: Vec<(&str, bool)> = changed
        .iter()
        .map(|f| (f.relative_path.as_str(), f.content.is_some()))
        .collect();
    assert_eq!(
        names,
        vec![
            ("src/a.rs", true),
            ("src/gone.rs", false),
            ("src/new.rs", true)
        ]
    );
    assert_eq!(
        changed[0].content.as_deref(),
        Some(b"a formatted".as_slice())
    );
}

/// A command whose client goes away mid-run leaves the copy exactly as it found it (#262):
/// the file it rewrote has its old text back, the files it created are gone with the
/// directory it made, and the file it deleted is there again, executable bit included.
#[tokio::test]
async fn a_command_whose_client_leaves_changes_nothing_in_the_copy() {
    let storage = tempfile::tempdir().unwrap();
    let workspace = storage.path().join("restore-ws");
    std::fs::create_dir_all(workspace.join("src")).unwrap();
    std::fs::write(workspace.join("src/a.rs"), "fn a() {}\n").unwrap();
    std::fs::write(workspace.join("src/gone.rs"), "fn gone() {}\n").unwrap();
    std::fs::write(workspace.join("run.sh"), "#!/bin/sh\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            workspace.join("run.sh"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
    let before = stamp_tree(&workspace);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    let (server, _) = listener.accept().await.unwrap();
    let storage_root = storage.path().to_path_buf();
    let metrics_dir = tempfile::tempdir().unwrap();
    let metrics = metrics::Metrics::new(metrics_dir.path().to_path_buf());
    let gateway = tokio::spawn(async move {
        let manager = WorkspaceManager::new();
        let mut framed = Framed::new(AnyStream::from(server), ProdCodeCodec::new());
        let req = ExecRequest {
            client_workspace_root: "/tmp/restore-ws".to_string(),
            base_workspace_name: Some("restore-ws".to_string()),
            command: [
                "sh",
                "-c",
                "printf 'fn a() { formatted }' > src/a.rs; printf new > src/new.rs; \
                     rm src/gone.rs run.sh; mkdir -p src/deep; printf x > src/deep/made.rs; \
                     echo ready; sleep 60",
            ]
            .map(str::to_string)
            .to_vec(),
            env: Vec::new(),
            timeout_secs: 120,
            pull_changes: true,
            subdir: None,
            client_agent: None,
            client_host: None,
        };
        run_exec(&storage_root, &metrics, &manager, &mut framed, req).await
    });

    let mut framed = Framed::new(client, ProdCodeCodec::new());
    let mut output = Vec::new();
    while !String::from_utf8_lossy(&output).contains("ready") {
        match tokio::time::timeout(std::time::Duration::from_secs(30), framed.next()).await {
            Ok(Some(Ok(WireMessage::ExecChunk(chunk)))) => {
                output.extend(chunk.data.unwrap_or_default())
            }
            other => panic!("no output from the command: {other:?}"),
        }
    }
    assert!(workspace.join("src/new.rs").is_file(), "the command ran");
    drop(framed);

    tokio::time::timeout(std::time::Duration::from_secs(30), gateway)
        .await
        .expect("the command was killed, not left to run out its sleep")
        .unwrap()
        .unwrap();
    assert_eq!(stamp_tree(&workspace), before);
    assert_eq!(
        std::fs::read_to_string(workspace.join("src/a.rs")).unwrap(),
        "fn a() {}\n"
    );
    assert!(!workspace.join("src/deep").exists());
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(workspace.join("run.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755);
    }
    assert!(workspace::stale_paths(&workspace).is_empty());
}

#[tokio::test]
async fn test_exec_fails_when_subdir_does_not_exist() {
    let storage = tempfile::tempdir().unwrap();
    let workspace = storage.path().join("test-ws");
    std::fs::create_dir_all(&workspace).unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    let (server, _) = listener.accept().await.unwrap();
    let storage_root = storage.path().to_path_buf();
    let metrics_dir = tempfile::tempdir().unwrap();
    let metrics = metrics::Metrics::new(metrics_dir.path().to_path_buf());
    let gateway = tokio::spawn(async move {
        let manager = WorkspaceManager::new();
        let mut framed = Framed::new(AnyStream::from(server), ProdCodeCodec::new());
        let req = ExecRequest {
            client_workspace_root: "/tmp/test-ws".to_string(),
            base_workspace_name: Some("test-ws".to_string()),
            command: vec!["pwd".to_string()],
            env: Vec::new(),
            timeout_secs: 10,
            pull_changes: false,
            subdir: Some("nonexistent_sub".to_string()),
            client_agent: None,
            client_host: None,
        };
        run_exec(&storage_root, &metrics, &manager, &mut framed, req).await
    });

    let mut framed = Framed::new(client, ProdCodeCodec::new());
    let exit = match tokio::time::timeout(std::time::Duration::from_secs(10), framed.next()).await {
        Ok(Some(Ok(WireMessage::ExecExit(exit)))) => exit,
        other => panic!("expected ExecExit, got: {other:?}"),
    };
    assert!(
        exit.error
            .as_deref()
            .unwrap_or_default()
            .contains("does not exist")
    );
    gateway.await.unwrap().unwrap();
}

/// A file that a client sync delivered while the command ran is the checkout's text and is
/// left alone; one the command changed again after it arrived is removed and reported stale;
/// one only the command changed gets its old bytes back (#262).
#[test]
fn a_restore_keeps_what_a_sync_delivered_during_the_command() {
    let storage = tempfile::tempdir().unwrap();
    let root = storage.path();
    for name in ["cmd.txt", "synced.txt", "both.txt"] {
        std::fs::write(root.join(name), "old\n").unwrap();
    }
    let before = snapshot_tree_within(root, RESTORE_MAX_FILE, RESTORE_BUDGET);
    std::fs::write(root.join("cmd.txt"), "the command's\n").unwrap();
    std::fs::write(root.join("synced.txt"), "the client's\n").unwrap();
    std::fs::write(root.join("both.txt"), "the command's, after the sync\n").unwrap();
    let synced = std::collections::HashMap::from([
        (
            "synced.txt".to_string(),
            Some(content_hash(b"the client's\n")),
        ),
        (
            "both.txt".to_string(),
            Some(content_hash(b"the client's\n")),
        ),
    ]);

    let restored = restore_tree(root, &before, &synced);

    assert_eq!(
        std::fs::read_to_string(root.join("cmd.txt")).unwrap(),
        "old\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("synced.txt")).unwrap(),
        "the client's\n"
    );
    assert!(!root.join("both.txt").exists());
    assert_eq!(restored.stale, vec!["both.txt".to_string()]);
    let mut files: Vec<&str> = restored
        .files
        .iter()
        .map(|f| f.relative_path.as_str())
        .collect();
    files.sort();
    assert_eq!(files, vec!["both.txt", "cmd.txt"]);
}
