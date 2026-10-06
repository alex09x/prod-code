/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_protocol::{ScrubSecrets, ShadowHypothesisResult};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::io::AsyncReadExt;

use super::staging::failed;
use super::tail_buffer::TailBuffer;
use super::types::{Ending, Job};

/// Spawns `cmd`, keeps the tail of its output, enforces the timeout and the cancel signal.
pub(crate) async fn run_child(
    mut cmd: tokio::process::Command,
    job: &Job,
    mut cancel: tokio::sync::watch::Receiver<bool>,
) -> ShadowHypothesisResult {
    // The cluster's secret credentials (token and TLS material) are never given to executed commands (#402, Phase 5.6).
    cmd.scrub_cluster_secrets();
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    // Own process group, so the timeout and a client disconnect take down the whole tree.
    #[cfg(unix)]
    cmd.process_group(0);
    let start = Instant::now();
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => return failed(&job.name, format!("failed to start: {e}")),
    };
    let tail = Arc::new(Mutex::new(TailBuffer::new(job.tail_limit)));
    let mut readers = Vec::new();
    if let Some(out) = child.stdout.take() {
        readers.push(spawn_reader(out, Arc::clone(&tail)));
    }
    if let Some(err) = child.stderr.take() {
        readers.push(spawn_reader(err, Arc::clone(&tail)));
    }
    let ending = tokio::select! {
        status = child.wait() => Ending::Exited(status.ok()),
        _ = tokio::time::sleep(job.timeout) => Ending::TimedOut,
        _ = cancel.changed() => Ending::Cancelled,
    };
    let (status, timed_out, error) = match ending {
        Ending::Exited(status) => (status, false, None),
        Ending::TimedOut => {
            crate::kill_exec_tree(&mut child);
            (child.wait().await.ok(), true, None)
        }
        Ending::Cancelled => {
            crate::kill_exec_tree(&mut child);
            (
                child.wait().await.ok(),
                false,
                Some("cancelled: the client left".to_string()),
            )
        }
    };
    for reader in readers {
        let _ = reader.await;
    }
    let tail = tail.lock().unwrap_or_else(|e| e.into_inner());
    ShadowHypothesisResult {
        name: job.name.clone(),
        exit_code: status.and_then(|s| s.code()),
        duration_ms: start.elapsed().as_millis() as u64,
        timed_out,
        error,
        output_tail: Some(tail.to_output()),
        output_len: tail.total,
    }
}

pub(crate) fn spawn_reader<R>(
    mut reader: R,
    tail: Arc<Mutex<TailBuffer>>,
) -> tokio::task::JoinHandle<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            match reader.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => tail
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(&buf[..n]),
            }
        }
    })
}
