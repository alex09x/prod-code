/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Metrics collection, ring buffer, disk persistence, and aggregation.

pub mod event;
pub mod storage;
pub mod summary;

#[cfg(test)]
mod tests;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub use event::Event;
use prod_code_protocol::MetricsResponse;
pub use storage::run_writer;
use summary::Summary;

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

const RING_CAP: usize = 200_000;

pub struct Metrics {
    dir: PathBuf,
    ring: Mutex<VecDeque<Event>>,
    /// Events queued for the disk writer (rapidfire unbounded MPSC: `record` never blocks
    /// the response path on file I/O).
    tx: rapidfire::mpsc::Sender<Event>,
    rx: Mutex<Option<rapidfire::mpsc::Receiver<Event>>>,
}

impl Metrics {
    pub fn new(dir: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&dir);
        let (tx, rx) = rapidfire::mpsc::unbounded::<Event>();
        Self {
            dir,
            ring: Mutex::new(VecDeque::with_capacity(4096)),
            tx,
            rx: Mutex::new(Some(rx)),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The receiver for [`run_writer`]; taken once at startup.
    pub fn take_receiver(&self) -> Option<rapidfire::mpsc::Receiver<Event>> {
        self.rx.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    /// Records one event: into the ring now, to today's JSONL file by the writer task.
    pub fn record(&self, event: Event) {
        let _ = self.tx.try_send(event.clone());
        let mut ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        if ring.len() >= RING_CAP {
            ring.pop_front();
        }
        ring.push_back(event);
    }

    /// Aggregates the last `since_secs` (0 = all the ring holds). The part of the window older
    /// than the oldest event in memory comes from the daily files, so a restart does not erase
    /// what the node served before it (#305); the ring's own events are not read twice.
    pub fn summary(&self, node: &str, since_secs: u64) -> MetricsResponse {
        let ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        let cutoff = if since_secs == 0 {
            0
        } else {
            now_ms().saturating_sub(since_secs * 1000)
        };
        let mut sum = Summary::default();
        if since_secs > 0 {
            let oldest_in_memory = ring.front().map_or_else(now_ms, |e| e.ts_ms);
            if cutoff < oldest_in_memory {
                storage::for_each_stored(&self.dir, cutoff, oldest_in_memory, |ev| sum.add(&ev));
            }
        }
        for ev in ring.iter().filter(|e| e.ts_ms >= cutoff) {
            sum.add(ev);
        }
        sum.response(node, since_secs, ring.len() as u64)
    }
}
