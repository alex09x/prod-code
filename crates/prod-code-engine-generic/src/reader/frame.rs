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
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, Weak};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStderr, ChildStdin};
use tokio::sync::{Mutex, oneshot};

use crate::types::{FrameWrite, lock_unpoisoned};

pub(crate) fn spawn_stderr_reader(
    mut stderr: ChildStderr,
    stderr_tail: Arc<StdMutex<VecDeque<String>>>,
    stderr_done_tx: tokio::sync::watch::Sender<bool>,
) {
    tokio::spawn(async move {
        let mut buf = [0u8; 4096];
        let mut current_line = String::new();
        while let Ok(n) = stderr.read(&mut buf).await {
            if n == 0 {
                break;
            }
            let chunk = String::from_utf8_lossy(&buf[..n]);
            for c in chunk.chars() {
                if c == '\n' {
                    let mut tail = lock_unpoisoned(&stderr_tail);
                    while tail.len() >= 30
                        || tail.iter().map(|s| s.len()).sum::<usize>() + current_line.len()
                            > 16 * 1024
                    {
                        if tail.pop_front().is_none() {
                            break;
                        }
                    }
                    tail.push_back(std::mem::take(&mut current_line));
                } else if current_line.len() < 512 {
                    current_line.push(c);
                }
            }
        }
        if !current_line.is_empty() {
            let mut tail = lock_unpoisoned(&stderr_tail);
            while tail.len() >= 30
                || tail.iter().map(|s| s.len()).sum::<usize>() + current_line.len() > 16 * 1024
            {
                if tail.pop_front().is_none() {
                    break;
                }
            }
            tail.push_back(current_line);
        }
        let _ = stderr_done_tx.send(true);
    });
}

pub(crate) async fn write_frame_until(
    writer: &Arc<Mutex<ChildStdin>>,
    val: &serde_json::Value,
    deadline: tokio::time::Instant,
    method: &str,
    child: &Weak<StdMutex<Child>>,
    pending: &Weak<StdMutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>>,
    is_alive: &Weak<AtomicBool>,
) -> Result<()> {
    let body = val.to_string();
    let frame = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
    let mut sin = tokio::time::timeout_at(deadline, writer.lock())
        .await
        .with_context(|| format!("Timeout waiting to send LSP message '{method}'"))?;
    if !is_alive
        .upgrade()
        .is_some_and(|alive| alive.load(Ordering::Acquire))
    {
        anyhow::bail!("Language server process has exited before message '{method}'");
    }
    // Declared after `sin`, so cancellation retires the process before unlocking stdin.
    let mut frame_write = FrameWrite {
        child: child.clone(),
        pending: pending.clone(),
        is_alive: is_alive.clone(),
        started: true,
        complete: false,
    };
    tokio::time::timeout_at(deadline, sin.write_all(frame.as_bytes()))
        .await
        .with_context(|| format!("Timeout writing LSP message '{method}'"))?
        .with_context(|| format!("Failed to write LSP message '{method}'"))?;
    tokio::time::timeout_at(deadline, sin.flush())
        .await
        .with_context(|| format!("Timeout flushing LSP message '{method}'"))?
        .with_context(|| format!("Failed to flush LSP message '{method}'"))?;
    frame_write.complete = true;
    Ok(())
}
