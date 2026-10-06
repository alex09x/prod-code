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
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

pub const SHARED_OUTPUT_CAPACITY: usize = 64;
pub const SHARED_OUTPUT_BATCH: usize = 64;
pub const SHARED_OUTPUT_WRITE_BUDGET: Duration = Duration::from_secs(2);
pub const SHARED_OUTPUT_TEARDOWN_BUDGET: Duration = Duration::from_secs(3);

#[doc(hidden)]
pub static ACTIVE_SHARED_OUTPUT_WRITERS: AtomicUsize = AtomicUsize::new(0);

pub struct SharedOutputFrame {
    pub message: WireMessage,
    pub deadline: tokio::time::Instant,
}

#[derive(Clone)]
pub struct SharedOutputSender {
    inner: rapidfire::mpsc::Sender<SharedOutputFrame>,
    write_budget: Duration,
}

#[derive(Debug)]
pub enum SharedOutputSendError {
    Closed,
    Deadline,
}

impl SharedOutputSender {
    pub fn new(inner: rapidfire::mpsc::Sender<SharedOutputFrame>, write_budget: Duration) -> Self {
        Self {
            inner,
            write_budget,
        }
    }

    pub async fn send(
        &self,
        message: WireMessage,
    ) -> std::result::Result<(), SharedOutputSendError> {
        let deadline = tokio::time::Instant::now() + self.write_budget;
        let frame = SharedOutputFrame { message, deadline };
        match tokio::time::timeout_at(deadline, self.inner.send(frame)).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) => Err(SharedOutputSendError::Closed),
            Err(_) => {
                // One expired producer expires the generation. This wakes every queue waiter and
                // prevents later notifications from repeatedly extending a dead client's life.
                self.close();
                Err(SharedOutputSendError::Deadline)
            }
        }
    }

    pub fn close(&self) {
        self.inner.close();
    }
}

pub struct SharedWriterLifetime {
    output: SharedOutputSender,
}

impl SharedWriterLifetime {
    pub fn start(output: SharedOutputSender) -> Self {
        ACTIVE_SHARED_OUTPUT_WRITERS.fetch_add(1, Ordering::Relaxed);
        Self { output }
    }
}

impl Drop for SharedWriterLifetime {
    fn drop(&mut self) {
        self.output.close();
        ACTIVE_SHARED_OUTPUT_WRITERS.fetch_sub(1, Ordering::Relaxed);
    }
}

pub struct OwnedJoin<T> {
    task: Option<tokio::task::JoinHandle<T>>,
}

impl<T> OwnedJoin<T> {
    pub fn new(task: tokio::task::JoinHandle<T>) -> Self {
        Self { task: Some(task) }
    }

    pub fn task_mut(&mut self) -> &mut tokio::task::JoinHandle<T> {
        self.task.as_mut().expect("owned task is live")
    }

    pub fn abort(&self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }

    pub fn clear_finished(&mut self) {
        let task = self.task.take().expect("owned task is live");
        debug_assert!(task.is_finished());
    }
}

impl<T> Drop for OwnedJoin<T> {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

pub fn flatten_writer_result(
    result: std::result::Result<Result<()>, tokio::task::JoinError>,
) -> Result<()> {
    result.map_err(|error| anyhow::anyhow!("shared output writer task failed: {error}"))?
}
