/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result};
use futures_util::SinkExt;
use prod_code_protocol::{AnyStream, ProdCodeCodec, WireMessage};
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::task::{AbortHandle, JoinHandle};
use tokio::time::{Instant, timeout_at};
use tokio_util::codec::Framed;

use super::command::frame;
use super::registry::{PendingEditorMessage, PendingServerFrame};

pub(crate) struct TaskAbortGuard(AbortHandle);

impl TaskAbortGuard {
    pub(crate) fn new<T>(task: &JoinHandle<T>) -> Self {
        Self(task.abort_handle())
    }
}

impl Drop for TaskAbortGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(crate) struct OwnedChild {
    pub(crate) child: tokio::process::Child,
    pub(crate) process_group: Option<i32>,
    pub(crate) leader_reaped: bool,
    pub(crate) group_retired: bool,
}

impl OwnedChild {
    pub(crate) fn new(child: tokio::process::Child) -> Self {
        #[cfg(unix)]
        let process_group = child.id().and_then(|pid| i32::try_from(pid).ok());
        #[cfg(not(unix))]
        let process_group = None;
        Self {
            child,
            process_group,
            leader_reaped: false,
            group_retired: false,
        }
    }

    pub(crate) fn retire_group(&mut self) {
        if self.group_retired {
            return;
        }
        self.group_retired = true;
        #[cfg(unix)]
        if let Some(group) = self.process_group {
            // The command was put in its own process group before spawn. A negative PID
            // targets only that owned group, including descendants which ignore shutdown.
            let _ = unsafe { libc::kill(-group, libc::SIGKILL) };
        }
        if !self.leader_reaped {
            let _ = self.child.start_kill();
        }
    }

    pub(crate) async fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        let result = self.child.wait().await;
        if result.is_ok() {
            self.leader_reaped = true;
        }
        result
    }

    pub(crate) async fn retire(&mut self, deadline: Instant) {
        // Reaping the direct child says nothing about descendants which still belong to the
        // exact process group created at spawn. Retire that group once on every exit path.
        self.retire_group();
        if self.leader_reaped {
            return;
        }
        if matches!(timeout_at(deadline, self.child.wait()).await, Ok(Ok(_))) {
            self.leader_reaped = true;
        }
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        self.retire_group();
        if self.leader_reaped {
            return;
        }
        // Cancellation cannot await. Give the exact child a short synchronous reap window;
        // kill_on_drop remains the final fallback if the platform has not reported it yet.
        for _ in 0..50 {
            match self.child.try_wait() {
                Ok(Some(_)) => {
                    self.leader_reaped = true;
                    break;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(2)),
                Err(_) => break,
            }
        }
    }
}

pub(crate) async fn write_server_frames(
    mut stdin: tokio::process::ChildStdin,
    mut input: rapidfire::mpsc::Receiver<PendingServerFrame>,
) -> Result<()> {
    while let Ok(pending) = input.recv().await {
        let bytes = frame(&pending.body);
        timeout_at(pending.deadline, async {
            stdin.write_all(&bytes).await?;
            stdin.flush().await
        })
        .await
        .context("editor server input write exceeded its deadline")?
        .context("writing an editor server input frame")?;
    }
    Ok(())
}

pub(crate) async fn write_editor_messages(
    mut socket: futures_util::stream::SplitSink<Framed<AnyStream, ProdCodeCodec>, WireMessage>,
    mut input: rapidfire::mpsc::Receiver<PendingEditorMessage>,
) -> Result<()> {
    while let Ok(pending) = input.recv().await {
        timeout_at(pending.deadline, socket.send(pending.message))
            .await
            .context("editor socket write exceeded its deadline")?
            .context("writing a message to the editor")?;
    }
    Ok(())
}

pub(crate) async fn finish_task(
    task: &mut JoinHandle<Result<()>>,
    already_finished: bool,
    deadline: Instant,
    session_id: u64,
    task_name: &'static str,
) {
    if already_finished {
        return;
    }
    match timeout_at(deadline, &mut *task).await {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(error))) => tracing::warn!(
            session_id,
            task = task_name,
            error = %error,
            "editor task failed during teardown"
        ),
        Ok(Err(error)) => tracing::warn!(
            session_id,
            task = task_name,
            %error,
            "editor task join failed during teardown"
        ),
        Err(_) => {
            tracing::warn!(
                session_id,
                task = task_name,
                "editor task exceeded the teardown deadline; aborting it"
            );
            task.abort();
            match task.await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => tracing::warn!(
                    session_id,
                    task = task_name,
                    error = %error,
                    "editor task failed while being aborted during teardown"
                ),
                Err(error) => tracing::warn!(
                    session_id,
                    task = task_name,
                    %error,
                    "editor task join failed after teardown abort"
                ),
            }
        }
    }
}
