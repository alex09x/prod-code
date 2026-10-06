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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, Weak};
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::{Child, ChildStdin};
use tokio::sync::{Mutex, Notify, OwnedMutexGuard};
use tokio::time::Instant;

use super::probe::{ProbeState, health_probe_sequence, lock_unpoisoned};
use super::request_history::{IssuedRequestHistory, IssuedRequestRegistration};

#[derive(Clone)]
pub(crate) struct FrameWriter {
    pub(crate) stdin: Arc<Mutex<ChildStdin>>,
    pub(crate) child: Weak<StdMutex<Child>>,
    pub(crate) is_alive: Arc<AtomicBool>,
    pub(crate) closed: Arc<Notify>,
    pub(crate) engine: Arc<str>,
    pub(crate) timeout: Duration,
    pub(crate) ordinary_epoch: Arc<AtomicU64>,
    pub(crate) last_activity: Arc<StdMutex<Instant>>,
    pub(crate) probe_state: Arc<StdMutex<ProbeState>>,
    pub(crate) issued_requests: Arc<StdMutex<IssuedRequestHistory>>,
    pub(crate) health_probe_id_prefix: Arc<str>,
}

impl FrameWriter {
    pub(crate) fn retire(&self) {
        self.is_alive.store(false, Ordering::Release);
        self.closed.notify_waiters();
        self.closed.notify_one();
        if let Some(child) = self.child.upgrade() {
            match child.lock() {
                Ok(mut child) => {
                    let _ = child.start_kill();
                }
                Err(error) => {
                    let mut child = error.into_inner();
                    let _ = child.start_kill();
                }
            }
        }
    }

    pub(crate) async fn send(&self, json_payload: &str) -> Result<()> {
        self.send_with_timeout(json_payload, self.timeout).await
    }

    pub(crate) async fn send_control(&self, json_payload: &str) -> Result<()> {
        self.write_frame_with_timeout(json_payload, self.timeout)
            .await
    }

    pub(crate) async fn send_with_timeout(
        &self,
        json_payload: &str,
        timeout: Duration,
    ) -> Result<()> {
        anyhow::ensure!(
            self.is_alive.load(Ordering::Acquire),
            "{} backend process has exited",
            self.engine
        );

        let ordinary_request = serde_json::from_str::<serde_json::Value>(json_payload)
            .ok()
            .filter(|value| {
                value.get("jsonrpc").and_then(serde_json::Value::as_str) == Some("2.0")
                    && value
                        .get("method")
                        .and_then(serde_json::Value::as_str)
                        .is_some()
                    && value.get("id").is_some_and(|id| !id.is_null())
            });
        if let Some(request) = &ordinary_request {
            anyhow::ensure!(
                health_probe_sequence(&request["id"], &self.health_probe_id_prefix).is_none(),
                "request id is reserved for private {} backend health probes",
                self.engine
            );
        }

        self.ordinary_epoch.fetch_add(1, Ordering::AcqRel);
        *lock_unpoisoned(&self.last_activity) = Instant::now();
        lock_unpoisoned(&self.probe_state).consecutive_timeouts = 0;
        let mut registration = ordinary_request.map(|value| {
            let token = lock_unpoisoned(&self.issued_requests).insert(value["id"].clone());
            IssuedRequestRegistration {
                history: Arc::downgrade(&self.issued_requests),
                token,
                committed: false,
            }
        });

        self.write_frame_with_timeout(json_payload, timeout).await?;
        if let Some(registration) = &mut registration {
            registration.committed = true;
        }
        Ok(())
    }

    pub(crate) async fn write_frame_with_timeout(
        &self,
        json_payload: &str,
        timeout: Duration,
    ) -> Result<()> {
        anyhow::ensure!(
            self.is_alive.load(Ordering::Acquire),
            "{} backend process has exited",
            self.engine
        );
        let deadline = Instant::now() + timeout;
        let stdin = tokio::time::timeout_at(deadline, Arc::clone(&self.stdin).lock_owned())
            .await
            .with_context(|| {
                format!(
                    "timed out after {timeout:?} waiting for {} backend writer",
                    self.engine
                )
            })?;
        anyhow::ensure!(
            self.is_alive.load(Ordering::Acquire),
            "{} backend process has exited",
            self.engine
        );

        let mut frame = FrameWrite::new(stdin, self.clone(), timeout);
        let header = format!("Content-Length: {}\r\n\r\n", json_payload.len());
        frame
            .write_all_until(deadline, header.as_bytes(), "header")
            .await?;
        frame
            .write_all_until(deadline, json_payload.as_bytes(), "payload")
            .await?;
        frame.flush_until(deadline).await?;
        frame.complete = true;
        Ok(())
    }
}

pub(crate) struct FrameWrite {
    pub(crate) stdin: OwnedMutexGuard<ChildStdin>,
    pub(crate) writer: FrameWriter,
    pub(crate) timeout: Duration,
    pub(crate) bytes_written: usize,
    pub(crate) faulted: bool,
    pub(crate) complete: bool,
}

impl FrameWrite {
    pub(crate) fn new(
        stdin: OwnedMutexGuard<ChildStdin>,
        writer: FrameWriter,
        timeout: Duration,
    ) -> Self {
        Self {
            stdin,
            writer,
            timeout,
            bytes_written: 0,
            faulted: false,
            complete: false,
        }
    }

    pub(crate) async fn write_all_until(
        &mut self,
        deadline: Instant,
        mut bytes: &[u8],
        part: &str,
    ) -> Result<()> {
        while !bytes.is_empty() {
            let written = match tokio::time::timeout_at(deadline, self.stdin.write(bytes)).await {
                Ok(Ok(0)) => {
                    self.faulted = true;
                    anyhow::bail!("{} backend writer returned zero bytes", self.writer.engine);
                }
                Ok(Ok(written)) => written,
                Ok(Err(error)) => {
                    self.faulted = true;
                    return Err(error).with_context(|| {
                        format!("failed writing {} backend LSP {part}", self.writer.engine)
                    });
                }
                Err(_) => anyhow::bail!(
                    "timed out after {:?} writing {} backend LSP {part}",
                    self.timeout,
                    self.writer.engine
                ),
            };
            self.bytes_written += written;
            bytes = &bytes[written..];
        }
        Ok(())
    }

    pub(crate) async fn flush_until(&mut self, deadline: Instant) -> Result<()> {
        match tokio::time::timeout_at(deadline, self.stdin.flush()).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => {
                self.faulted = true;
                Err(error).with_context(|| {
                    format!("failed flushing {} backend LSP frame", self.writer.engine)
                })
            }
            Err(_) => anyhow::bail!(
                "timed out after {:?} flushing {} backend LSP frame",
                self.timeout,
                self.writer.engine
            ),
        }
    }
}

impl Drop for FrameWrite {
    fn drop(&mut self) {
        if !self.complete && (self.bytes_written != 0 || self.faulted) {
            // Poison the connection before the owned mutex guard releases it: queued writers
            // must never append a new frame after an incomplete one.
            self.writer.retire();
        }
    }
}
