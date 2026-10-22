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
    let exited = Arc::new(std::sync::Mutex::new(false));
    let (exit_tx, mut exit_rx) = tokio::sync::oneshot::channel();
    {
        let exited = exited.clone();
        tokio::task::spawn_blocking(move || {
            let status = wait_with_usage(pid as i32, &exited);
            // Status describes the process, not later pipe draining or workspace recovery.
            drop(running);
            let _ = exit_tx.send(status);
            drop(child);
        });
    }
    let mut timed_out = false;
    let mut status = None;
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
            exit = &mut exit_rx, if status.is_none() => {
                status = Some(exit.ok().flatten());
            }
            _ = tokio::time::sleep_until(deadline) => {
                timed_out = true;
                kill_exec_group(pid, &exited);
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
        if client_left || (!chunks_open && status.is_some()) {
            break;
        }
    }
    if client_left || timed_out {
        kill_exec_group(pid, &exited);
        drop(rx);
        if status.is_none() {
            status = Some(exit_rx.await.ok().flatten());
        }
        for reader in &readers {
            reader.abort();
        }
    }
    for reader in readers {
        let _ = reader.await;
    }
    Ok(ExecCompletion {
        timed_out,
        client_left,
        status: status.flatten(),
        accumulator,
    })
}

#[cfg(test)]
#[path = "exec_stream_tests.rs"]
mod tests;
