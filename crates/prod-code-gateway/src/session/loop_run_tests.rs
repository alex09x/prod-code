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
use crate::workspace::shared::SharedWorkspace;
use std::sync::Arc;

#[test]
fn shared_worktree_invalidation_only_retires_stale_workspace_views() {
    let temp = tempfile::tempdir().unwrap();
    let base_root = temp.path().join("base");
    let worktree_root = temp.path().join("worktree");
    std::fs::create_dir_all(&base_root).unwrap();
    std::fs::create_dir_all(&worktree_root).unwrap();
    std::fs::write(base_root.join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
    std::fs::write(
        worktree_root.join("Cargo.toml"),
        "[package]\nname = \"worktree\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    let base = Arc::new(SharedWorkspace::new(
        base_root,
        "rust".to_string(),
        None,
        None,
        None,
        None,
    ));
    let worktree = Arc::new(SharedWorkspace::with_base(
        worktree_root,
        "rust".to_string(),
        None,
        None,
        None,
        None,
        Some(Arc::clone(&base)),
    ));
    let reason = Some(prod_code_protocol::WORKSPACE_MODEL_CHANGED_REASON);

    worktree.mark_unloaded();
    assert!(workspace_model_change_applies(&worktree, reason));
    assert!(!workspace_model_change_applies(&base, reason));

    base.mark_unloaded();
    assert!(workspace_model_change_applies(&base, reason));
    assert!(workspace_model_change_applies(&worktree, reason));
    assert!(!workspace_model_change_applies(
        &worktree,
        Some("ordinary rebalance")
    ));
}
