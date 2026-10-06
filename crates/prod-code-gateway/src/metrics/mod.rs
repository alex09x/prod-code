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
pub mod inventory;
pub mod prometheus_format;
pub mod prometheus_push;
pub mod prometheus_server;
pub mod snapshot;
pub mod storage;
pub mod summary;

#[cfg(test)]
mod tests;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};

pub use event::{Event, classify_error, command_method, is_compilation_command, now_ms};
pub use inventory::collect_toolchain_inventory;
#[allow(unused_imports)]
pub use prometheus_format::format_prometheus_metrics;
pub use prometheus_push::run_prometheus_push_loop;
pub use prometheus_server::run_prometheus_server;
pub use snapshot::collect_host_snapshot;
pub use storage::{DEFAULT_METRICS_RETENTION_DAYS, MAX_METRICS_STORAGE_BYTES, run_writer};

pub use prod_code_protocol::{
    HostSnapshot, MetricsResponse, OperationMetric, TelemetryRecord, ToolchainInventory,
};
use summary::Summary;

const RING_CAP: usize = 200_000;
const SNAPSHOT_RING_CAP: usize = 1440; // 24 hours of 1-minute snapshots

pub struct Metrics {
    dir: PathBuf,
    node: RwLock<String>,
    ring: Mutex<VecDeque<Event>>,
    snapshots: Mutex<VecDeque<HostSnapshot>>,
    inventory: RwLock<Option<ToolchainInventory>>,
    /// Telemetry records queued for the disk writer (rapidfire unbounded MPSC: `record` never blocks
    /// the response path on file I/O).
    tx: rapidfire::mpsc::Sender<TelemetryRecord>,
    rx: Mutex<Option<rapidfire::mpsc::Receiver<TelemetryRecord>>>,
}

impl Metrics {
    pub fn new(dir: PathBuf) -> Self {
        Self::with_node(dir, "local")
    }

    pub fn with_node(dir: PathBuf, node: impl Into<String>) -> Self {
        let _ = std::fs::create_dir_all(&dir);
        let (tx, rx) = rapidfire::mpsc::unbounded::<TelemetryRecord>();
        Self {
            dir,
            node: RwLock::new(node.into()),
            ring: Mutex::new(VecDeque::with_capacity(4096)),
            snapshots: Mutex::new(VecDeque::with_capacity(SNAPSHOT_RING_CAP)),
            inventory: RwLock::new(None),
            tx,
            rx: Mutex::new(Some(rx)),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn set_node(&self, node: impl Into<String>) {
        if let Ok(mut n) = self.node.write() {
            *n = node.into();
        }
    }

    pub fn node(&self) -> String {
        self.node
            .read()
            .map(|n| n.clone())
            .unwrap_or_else(|_| "unknown".to_string())
    }

    /// The receiver for [`run_writer`]; taken once at startup.
    pub fn take_receiver(&self) -> Option<rapidfire::mpsc::Receiver<TelemetryRecord>> {
        self.rx.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    /// Records one event: into the in-memory ring and into today's JSONL file via disk writer.
    pub fn record(&self, event: Event) {
        let node = self.node();
        let op = event.to_operation_metric(&node);
        let _ = self.tx.try_send(TelemetryRecord::Operation(op));

        let mut ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        if ring.len() >= RING_CAP {
            ring.pop_front();
        }
        ring.push_back(event);
    }

    /// Records an operation directly as an [`OperationMetric`].
    pub fn record_operation(&self, metric: OperationMetric) {
        let _ = self.tx.try_send(TelemetryRecord::Operation(metric));
    }

    /// Records a host/gateway resource snapshot.
    pub fn record_snapshot(&self, snapshot: HostSnapshot) {
        let _ = self
            .tx
            .try_send(TelemetryRecord::Snapshot(snapshot.clone()));
        let mut ring = self.snapshots.lock().unwrap_or_else(|e| e.into_inner());
        if ring.len() >= SNAPSHOT_RING_CAP {
            ring.pop_front();
        }
        ring.push_back(snapshot);
    }

    /// Records or updates the toolchain inventory.
    pub fn record_inventory(&self, inventory: ToolchainInventory) {
        let _ = self
            .tx
            .try_send(TelemetryRecord::Inventory(inventory.clone()));
        if let Ok(mut inv) = self.inventory.write() {
            *inv = Some(inventory);
        }
    }

    /// Retrieves the most recent host telemetry snapshot, if any.
    pub fn latest_snapshot(&self) -> Option<HostSnapshot> {
        let ring = self.snapshots.lock().unwrap_or_else(|e| e.into_inner());
        ring.back().cloned()
    }

    /// Returns the most recent `count` snapshots in chronological order.
    pub fn recent_snapshots(&self, count: usize) -> Vec<HostSnapshot> {
        let ring = self.snapshots.lock().unwrap_or_else(|e| e.into_inner());
        let skip = ring.len().saturating_sub(count);
        ring.iter().skip(skip).cloned().collect()
    }

    /// Iterates through all currently buffered events in the in-memory ring.
    pub fn for_each_ring_event(&self, mut f: impl FnMut(&Event)) {
        let ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        for ev in ring.iter() {
            f(ev);
        }
    }

    /// Returns the cached toolchain inventory.
    pub fn toolchain_inventory(&self) -> Option<ToolchainInventory> {
        self.inventory.read().ok().and_then(|i| i.clone())
    }

    /// Resolves the compiler and tool version associated with a compilation command, if known.
    pub fn resolve_compiler(&self, command: &str) -> Option<String> {
        let first = command.split_whitespace().next()?;
        let base = std::path::Path::new(first)
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_else(|| first.to_string());

        let target_tool = match base.as_str() {
            "cargo" | "rustc" => "rustc",
            "go" => "go",
            "clang" | "clang++" | "gcc" | "g++" | "cc" | "c++" => "clang",
            "swift" | "swiftc" => "swift",
            "tsc" => "tsc",
            "npx" if command.split_whitespace().any(|a| a == "tsc") => "tsc",
            "zig" => "zig",
            "javac" => "javac",
            "kotlinc" => "kotlinc",
            "scalac" => "scala",
            "dotnet" => "dotnet",
            _ => return None,
        };

        if let Some(inv) = self.toolchain_inventory() {
            for engine in &inv.engines {
                for tv in &engine.toolchains {
                    if tv.tool.eq_ignore_ascii_case(target_tool) {
                        return Some(format!("{target_tool} {}", tv.version));
                    }
                }
            }
        }
        Some(target_tool.to_string())
    }

    /// Prunes expired metric files from disk according to retention policy.
    pub fn prune_retention(&self, retention_days: u64, max_bytes: u64) -> usize {
        storage::prune_expired_metrics(&self.dir, retention_days, max_bytes)
    }

    /// Aggregates the last `since_secs` (0 = all the ring holds).
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

        let snapshots = self.recent_snapshots(60);
        let inventory = self.toolchain_inventory();

        sum.response(node, since_secs, ring.len() as u64, snapshots, inventory)
    }
}
