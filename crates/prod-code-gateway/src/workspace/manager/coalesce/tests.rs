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
use std::future::Future;
use std::pin::Pin;

mod worktrees;

#[tokio::test]
async fn waiting_sessions_receive_the_leaders_failure_close_or_published_workspace() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("workspace");
    let base = temp.path().join("base");
    for worktree in [false, true] {
        for outcome in ["failure", "closed", "ready"] {
            let manager = Arc::new(WorkspaceManager::new());
            let key = WorkspaceKey(root.clone());
            let (tx, _) = broadcast::channel(1);
            manager
                .workspaces
                .write()
                .await
                .insert(key.clone(), LoadState::Loading(tx.clone()));
            let mut load: Pin<Box<dyn Future<Output = Result<WorkspaceLease>> + '_>> = if worktree {
                Box::pin(manager.get_or_load_worktree(&root, &base))
            } else {
                Box::pin(manager.get_or_load_direct(&root, "rust"))
            };
            tokio::select! {
                biased;
                _ = &mut load => panic!("a follower must wait for its leader"),
                _ = tokio::task::yield_now() => {}
            }
            let ready = Arc::new(SharedWorkspace::new(
                root.clone(),
                "rust".into(),
                None,
                None,
                None,
                None,
            ));
            match outcome {
                "failure" => {
                    assert_eq!(tx.send(Err("scripted refusal".into())).ok(), Some(1));
                }
                "closed" => {
                    manager.workspaces.write().await.remove(&key);
                    drop(tx);
                }
                _ => {
                    manager
                        .workspaces
                        .write()
                        .await
                        .insert(key, LoadState::Ready(Arc::clone(&ready)));
                    assert_eq!(tx.send(Ok(Arc::clone(&ready))).ok(), Some(1));
                }
            }
            match load.await {
                Ok(lease) => {
                    assert_eq!(outcome, "ready");
                    assert!(Arc::ptr_eq(lease.workspace(), &ready));
                    assert_eq!(ready.active_sessions.load(Ordering::Relaxed), 1);
                    drop(lease);
                    assert_eq!(ready.active_sessions.load(Ordering::Relaxed), 0);
                }
                Err(error) => assert_eq!(
                    error.to_string(),
                    if outcome == "failure" {
                        "Workspace load failed: scripted refusal"
                    } else {
                        "Leader dropped load broadcast: channel closed"
                    }
                ),
            }
        }
    }
}

#[tokio::test]
async fn refused_base_load_releases_the_worktree_loading_slot() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().join("base");
    let root = temp.path().join("copy");
    let admission = Arc::new(crate::admission::Admission::with_probe(
        crate::admission::scripted_probe(vec![(0, 100)]),
        2048,
        std::time::Duration::ZERO,
    ));
    let manager = Arc::new(WorkspaceManager::with_admission(admission));
    let expected = manager
        .get_or_load_direct(&base, "rust")
        .await
        .err()
        .unwrap()
        .to_string();
    let error = manager
        .get_or_load_worktree(&root, &base)
        .await
        .err()
        .unwrap();
    assert_eq!(error.to_string(), expected);
    assert!(
        !manager
            .workspaces
            .read()
            .await
            .contains_key(&WorkspaceKey(root))
    );
}
