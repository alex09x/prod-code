/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use crate::workspace::loader::LoadState;
use crate::workspace::manager::WorkspaceManager;
use crate::workspace::paths::{
    resolve_server_workspace, server_workspace_path, split_worktree_base, worktree_suffix,
};
use crate::workspace::shared::SharedWorkspace;
use crate::workspace::types::WorkspaceKey;

#[test]
fn test_resolve_server_workspace_generic_worktree_mapping() {
    let temp_dir = tempfile::tempdir().unwrap();
    let storage = temp_dir.path();

    // 1. Nested runner worktree container: .../worktrees/<workspace_name>/task-123/attempt-0
    let wt1 = "/Volumes/worktrees/worktrees/repo-alpha/task-1531/attempt-0";
    let res1 = resolve_server_workspace(storage, wt1, None);
    assert_eq!(
        res1,
        storage.join(format!("repo-alpha{}", worktree_suffix(wt1)))
    );
    assert!(res1.is_dir(), "Workspace directory must be auto-created");
    let wt1b = "/Volumes/worktrees/worktrees/repo-alpha/task-1532/attempt-0";
    assert_ne!(resolve_server_workspace(storage, wt1b, None), res1);

    // 2. In-repo dot-worktrees pattern: .../project-beta/.worktrees/branch-1
    let wt2 = "/home/dev/projects/project-beta/.worktrees/branch-1";
    let res2 = resolve_server_workspace(storage, wt2, None);
    assert_eq!(
        res2,
        storage.join(format!("project-beta{}", worktree_suffix(wt2)))
    );
    assert!(res2.is_dir());

    // 3. Worktree with explicit base name provided by client
    let wt3 = "/Users/dev/scratch/temp-worktree";
    let res3 = resolve_server_workspace(storage, wt3, Some("core-service"));
    assert_eq!(res3, storage.join("core-service"));
    assert!(res3.is_dir());

    // 4. Standard repository folder
    let std_repo = "/Users/dev/workspace/payment-gateway";
    let res4 = resolve_server_workspace(storage, std_repo, None);
    assert_eq!(res4, storage.join("payment-gateway"));
    assert!(res4.is_dir());
}

#[cfg(unix)]
#[test]
fn test_resolve_server_workspace_canonicalizes_symlink_storage_root() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    let real_storage = temp.path().join("real-storage");
    std::fs::create_dir_all(&real_storage).unwrap();
    let storage_alias = temp.path().join("storage-alias");
    symlink(&real_storage, &storage_alias).unwrap();
    let client_root = "/home/dev/workspace/project";

    let target = server_workspace_path(&storage_alias, client_root, None);
    let resolved = resolve_server_workspace(&storage_alias, client_root, None);

    assert_eq!(resolved, std::fs::canonicalize(target).unwrap());
}

#[test]
fn test_split_worktree_base() {
    let temp_dir = tempfile::tempdir().unwrap();
    let storage = temp_dir.path();
    let base_dir = storage.join("my-service");
    std::fs::create_dir_all(&base_dir).unwrap();

    let wt_dir = storage.join("my-service--wt-a1b2c3d4");
    std::fs::create_dir_all(&wt_dir).unwrap();
    assert_eq!(split_worktree_base(&wt_dir), Some(base_dir.clone()));

    let nested_wt = wt_dir.join("crates").join("sub-crate");
    assert_eq!(
        split_worktree_base(&nested_wt),
        Some(base_dir.join("crates").join("sub-crate"))
    );

    assert_eq!(split_worktree_base(&base_dir), None);

    let non_existent_base = storage.join("other--wt-12345678");
    assert_eq!(split_worktree_base(&non_existent_base), None);

    // Test standard git worktree with `.git` file pointing to base repo
    let git_base = storage.join("git-repo");
    let git_dir = git_base.join(".git");
    let wt_meta = git_dir.join("worktrees").join("branch-wt");
    std::fs::create_dir_all(&wt_meta).unwrap();

    let git_wt = storage.join("git-repo-wt");
    std::fs::create_dir_all(&git_wt).unwrap();
    std::fs::write(
        git_wt.join(".git"),
        format!("gitdir: {}\n", wt_meta.display()),
    )
    .unwrap();

    assert_eq!(split_worktree_base(&git_wt), Some(git_base.clone()));

    // Test nested package inside standard git worktree (no local .git file, found in ancestor)
    let nested_git_wt = git_wt.join("crates").join("sub-crate");
    let nested_git_base = git_base.join("crates").join("sub-crate");
    std::fs::create_dir_all(&nested_git_wt).unwrap();
    std::fs::create_dir_all(&nested_git_base).unwrap();
    assert_eq!(split_worktree_base(&nested_git_wt), Some(nested_git_base));
}

#[tokio::test]
async fn test_worktree_shares_base_rust_engine() {
    let temp_dir = tempfile::tempdir().unwrap();
    let storage = temp_dir.path();
    let base_dir = storage.join("sample-repo");
    std::fs::create_dir_all(base_dir.join("src")).unwrap();
    std::fs::write(
        base_dir.join("Cargo.toml"),
        "[package]\nname = \"sample-repo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        base_dir.join("src/lib.rs"),
        "pub fn base_func() -> u32 { 42 }\n",
    )
    .unwrap();

    let wt_dir = storage.join(format!("sample-repo{}", worktree_suffix("client-wt-path")));
    std::fs::create_dir_all(wt_dir.join("src")).unwrap();
    std::fs::write(
        wt_dir.join("Cargo.toml"),
        "[package]\nname = \"sample-repo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        wt_dir.join("src/lib.rs"),
        "pub fn base_func() -> u32 { 100 }\n",
    )
    .unwrap();

    let manager = Arc::new(WorkspaceManager::new());

    let base_lease = manager.get_or_load(&base_dir, "rust").await.unwrap();
    let base_ws = Arc::clone(base_lease.workspace());
    assert!(base_lease.rust_engine.is_some());
    assert!(base_lease.base_workspace.is_none());

    let wt_lease = manager.get_or_load(&wt_dir, "rust").await.unwrap();
    assert!(wt_lease.rust_engine.is_some());
    assert!(wt_lease.base_workspace.is_some());

    // Same underlying Arc<Mutex<RustEngine>>
    let base_eng = base_lease.rust_engine.as_ref().unwrap();
    let wt_eng = wt_lease.rust_engine.as_ref().unwrap();
    assert!(Arc::ptr_eq(base_eng, wt_eng));

    // Engine has worktree attached
    assert!(base_eng.lock().await.has_worktree(&wt_dir));

    // Worktree does not duplicate memory reclaimable
    assert_eq!(wt_lease.reclaimable(manager.admission()), 0);

    // Attached worktree increments base's attached_worktrees count
    assert_eq!(base_ws.attached_worktrees.load(Ordering::Relaxed), 1);
    // Base is not reclaimable while attached worktree exists
    assert_eq!(base_ws.reclaimable(manager.admission()), 0);

    // Validation view forwards to base validation and keeps worktree attached
    let wt_val = wt_lease
        .workspace()
        .validation_view(manager.admission())
        .await
        .unwrap();
    assert!(wt_val.base_workspace.is_some());
    assert!(Arc::ptr_eq(
        wt_val.base_workspace.as_ref().unwrap(),
        base_lease.workspace()
    ));

    // Active worktree lease pins base against eviction even if base_lease drops
    drop(base_lease);
    assert_eq!(base_ws.active_sessions.load(Ordering::Relaxed), 1);
    let early_evict = manager.evict_idle(Duration::from_secs(0)).await;
    assert!(
        early_evict.is_empty(),
        "base must not be evicted while worktree is active"
    );

    // Dropping worktree lease frees sessions, but attached_worktrees protects base until worktree is evicted
    drop(wt_lease);
    assert_eq!(base_ws.active_sessions.load(Ordering::Relaxed), 0);
    assert_eq!(base_ws.attached_worktrees.load(Ordering::Relaxed), 1);

    // Evicting idle worktrees detaches overlay and frees base
    let evicted = manager.evict_idle(Duration::from_secs(0)).await;
    assert!(evicted.contains(&wt_dir));
    assert_eq!(base_ws.attached_worktrees.load(Ordering::Relaxed), 0);

    // Now base has 0 attached worktrees and can be evicted
    let evicted_base = manager.evict_idle(Duration::from_secs(0)).await;
    assert!(evicted_base.contains(&base_dir));
}

#[tokio::test]
async fn test_trigger_rebalance_exact_name_matching_and_worktree() {
    let manager = WorkspaceManager::new();
    let path_shop = PathBuf::from("/work/shop");
    let path_shopper = PathBuf::from("/work/shopper");
    let path_shop_wt = PathBuf::from("/work/shop--wt1");

    let ws_shop = Arc::new(SharedWorkspace::new(
        path_shop.clone(),
        "rust".to_string(),
        None,
        None,
        None,
        None,
    ));
    let ws_shopper = Arc::new(SharedWorkspace::new(
        path_shopper.clone(),
        "rust".to_string(),
        None,
        None,
        None,
        None,
    ));
    let ws_shop_wt = Arc::new(SharedWorkspace::with_base(
        path_shop_wt.clone(),
        "rust".to_string(),
        None,
        None,
        None,
        None,
        Some(Arc::clone(&ws_shop)),
    ));

    let mut rx_shop = ws_shop.subscribe_rebalance();
    let mut rx_shopper = ws_shopper.subscribe_rebalance();
    let mut rx_shop_wt = ws_shop_wt.subscribe_rebalance();

    {
        let mut guard = manager.workspaces.write().await;
        guard.insert(
            WorkspaceKey(path_shop),
            LoadState::Ready(Arc::clone(&ws_shop)),
        );
        guard.insert(
            WorkspaceKey(path_shopper),
            LoadState::Ready(Arc::clone(&ws_shopper)),
        );
        guard.insert(
            WorkspaceKey(path_shop_wt),
            LoadState::Ready(Arc::clone(&ws_shop_wt)),
        );
    }

    // Rebalance "shop" to node-2:2026
    let notified = manager
        .trigger_rebalance_by_name("shop", "node-2:2026".to_string(), Some("test".to_string()))
        .await;
    assert!(notified >= 1);

    // ws_shop and ws_shop_wt share the rebalance broadcast channel, so both receive redirect
    let (target, reason) = rx_shop.try_recv().expect("shop must receive redirect");
    assert_eq!(target, "node-2:2026");
    assert_eq!(reason.as_deref(), Some("test"));

    let (target_wt, reason_wt) = rx_shop_wt
        .try_recv()
        .expect("shop worktree must receive redirect");
    assert_eq!(target_wt, "node-2:2026");
    assert_eq!(reason_wt.as_deref(), Some("test"));

    // ws_shopper must NOT receive redirect (preventing substring false positive)
    assert!(
        rx_shopper.try_recv().is_err(),
        "shopper must NOT receive redirect when rebalancing shop"
    );
}
