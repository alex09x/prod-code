/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The checkout locks a tool call holds while it runs. A call with a deadline stops waiting for
//! them at that deadline (#1814): a write that holds the checkout for minutes must not keep a
//! 300 ms navigation call waiting for minutes.

use std::path::Path;

use super::super::workspace_exec_lock;
use super::super::workspace_writer::{
    WorkspaceMutationGuard, workspace_mutation_lock, workspace_writer_gate,
};
use crate::sync::types::{QUEUED_FOR_WRITER, set_active_sync_stage};

/// Held for the whole call; dropping it releases the checkout.
pub(super) struct CallLocks {
    _exec: Option<tokio::sync::OwnedMutexGuard<()>>,
    _writer: Option<tokio::sync::OwnedMutexGuard<()>>,
    _mutation: Option<WorkspaceMutationGuard>,
}

/// Takes the locks `tool_name` needs on `root`, in the order every caller takes them. `None`
/// when `deadline` passed first; while it waits, the call's stage says it is queued behind a
/// workspace write, so a budget that ends meanwhile is reported as a queue wait.
pub(super) async fn acquire(
    root: &Path,
    tool_name: &str,
    read_only: bool,
    workspace_command: bool,
    deadline: Option<tokio::time::Instant>,
) -> Option<CallLocks> {
    set_active_sync_stage(Some(QUEUED_FOR_WRITER));
    let taken = within(
        deadline,
        take(root, tool_name, read_only, workspace_command),
    )
    .await;
    set_active_sync_stage(None);
    taken
}

async fn within<F: std::future::Future>(
    deadline: Option<tokio::time::Instant>,
    fut: F,
) -> Option<F::Output> {
    match deadline {
        Some(deadline) => tokio::time::timeout_at(deadline, fut).await.ok(),
        None => Some(fut.await),
    }
}

async fn take(root: &Path, tool_name: &str, read_only: bool, workspace_command: bool) -> CallLocks {
    let exec = if !read_only || workspace_command {
        Some(workspace_exec_lock(root).lock_owned().await)
    } else {
        None
    };
    // Commands may write back files, so serialize them with other writers even while
    // readers proceed during the remote execution phase.
    let writer = if read_only {
        None
    } else {
        Some(workspace_writer_gate(root).lock_owned().await)
    };
    let lock = workspace_mutation_lock(root);
    let mutation = if tool_name == "code_exec" {
        None // its writeback takes this lock only while applying changes
    } else if read_only {
        Some(WorkspaceMutationGuard::Read(lock.read_owned().await))
    } else {
        Some(WorkspaceMutationGuard::Write(lock.write_owned().await))
    };
    CallLocks {
        _exec: exec,
        _writer: writer,
        _mutation: mutation,
    }
}
