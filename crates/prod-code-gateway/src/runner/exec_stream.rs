/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::*;

pub(crate) struct ExecCompletion {
    pub timed_out: bool,
    pub client_left: bool,
    pub status: Option<(i32, prod_code_protocol::ExecUsage)>,
    pub accumulator: JsonStreamAccumulator,
}

/// Owns the command's process group while its stream runs. The waiter leaves the exited
/// leader unreaped until this guard is gone, so every signal the stream sends reaches its own
/// group and never a later one that reused the number (#1545). When the stream's future is
/// dropped or cancelled before it finishes, the group is killed first (#1536); the waiter then
/// reaps the leader.
struct GroupGuard {
    pid: u32,
    armed: bool,
    /// Dropped after `drop` has run, which lets the waiter reap the leader.
    _release: std::sync::mpsc::Sender<()>,
}

impl Drop for GroupGuard {
    fn drop(&mut self) {
        if self.armed {
            kill_exec_group(self.pid);
        }
    }
}

/// Shared process/connection lifecycle for plain and structured execution.
pub(crate) async fn stream_exec(
    mut child: std::process::Child,
    running: RunningEntry,
    framed: &mut Framed<AnyStream, ProdCodeCodec>,
    deadline: tokio::time::Instant,
    structured: Option<(RemoteExecLanguage, bool)>,
) -> Result<ExecCompletion> {
    let (tx, mut rx) = rapidfire::mpsc::bounded::<ExecChunk>(256);
    let readers = spawn_pipe_readers(&mut child, tx.clone());
    drop(tx);
    let pid = child.id();
    let (release, released) = std::sync::mpsc::channel::<()>();
    let mut group = GroupGuard {
        pid,
        armed: true,
        _release: release,
    };
    let (exit_tx, mut exit_rx) = tokio::sync::oneshot::channel::<()>();
    let (status_tx, status_rx) = tokio::sync::oneshot::channel();
    tokio::task::spawn_blocking(move || {
        let exited = wait_for_exit(pid as i32);
        // Status describes the process, not later pipe draining or workspace recovery.
        drop(running);
        let _ = exit_tx.send(());
        // The zombie holds the group's number until the guard is gone; nothing is ever sent,
        // so `recv` returns when it drops.
        let _ = released.recv();
        let status = if exited {
            exec_shim::reap_with_usage(pid as i32)
        } else {
            None
        };
        let _ = status_tx.send(status);
        drop(child);
    });
    let mut timed_out = false;
    let mut exited = false;
    let mut chunks_open = true;
    let mut client_left = false;
    let mut accumulator = JsonStreamAccumulator::new();
    loop {
        tokio::select! {
            chunk = rx.recv(), if chunks_open => match chunk {
                Ok(chunk) => {
                    let is_stderr = chunk.stderr;
                    let data = chunk.data.clone();
                    let message = if structured.is_some() {
                        WireMessage::RemoteExecStream(RemoteExecStream::Chunk(chunk))
                    } else {
                        WireMessage::ExecChunk(chunk)
                    };
                    client_left = !matches!(
                        tokio::time::timeout_at(deadline, framed.send(message)).await,
                        Ok(Ok(()))
                    );
                    if !client_left && !is_stderr
                        && let Some((language, true)) = structured
                        && let Some(data) = data {
                            client_left = !matches!(
                                tokio::time::timeout_at(deadline, accumulator.push_chunk(&data, language, framed)).await,
                                Ok(Ok(true))
                            );
                        }
                }
                Err(_) => chunks_open = false,
            },
            _ = &mut exit_rx, if !exited => exited = true,
            _ = tokio::time::sleep_until(deadline) => {
                timed_out = true;
                kill_exec_group(pid);
                break;
            }
            // Descendants may hold pipes after the direct child exits. Keep watching the
            // connection so its client can still cancel the drain.
            incoming = framed.next() => match incoming {
                Some(Ok(WireMessage::Ping)) => {
                    client_left = !matches!(
                        tokio::time::timeout_at(deadline, framed.send(WireMessage::Pong)).await,
                        Ok(Ok(()))
                    );
                }
                Some(Ok(WireMessage::Disconnect { .. })) | Some(Err(_)) | None => client_left = true,
                _ => {}
            }
        }
        if client_left || (!chunks_open && exited) {
            break;
        }
    }
    if client_left || timed_out {
        kill_exec_group(pid);
        drop(rx);
        for reader in &readers {
            reader.abort();
        }
    }
    for reader in readers {
        let _ = reader.await;
    }
    // Finished on its own terms: exit, timeout or a departed client already handled the group.
    // Only now may the waiter reap the leader and free the group's number.
    group.armed = false;
    drop(group);
    let status = status_rx.await.ok().flatten();
    Ok(ExecCompletion {
        timed_out,
        client_left,
        status,
        accumulator,
    })
}

#[cfg(test)]
#[path = "exec_stream_tests.rs"]
mod tests;
