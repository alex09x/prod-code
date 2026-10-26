/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::storage::{for_each_stored, write_telemetry_batch};
use super::summary::Summary;
use super::{Event, command_method, now_ms};
use prod_code_protocol::{OperationMetric, TelemetryRecord};

fn operation(ts_ms: u64, category: &str, method: &str) -> OperationMetric {
    OperationMetric {
        ts_ms,
        node: "test-node".to_string(),
        category: category.to_string(),
        method: method.to_string(),
        engine: "rust".to_string(),
        duration_ms: 42,
        ok: true,
        error_class: None,
        items: 1,
        bytes: 0,
        exit_code: Some(0),
        agent: Some("test-agent".to_string()),
        host: Some("test-host".to_string()),
        workspace: Some("test-workspace".to_string()),
        compiler: None,
    }
}

fn summarize_persisted(operation: OperationMetric) -> prod_code_protocol::MetricsResponse {
    let temp = tempfile::tempdir().expect("metrics temp dir");
    let dir = temp.path().join("metrics");
    std::fs::create_dir_all(&dir).expect("metrics dir");
    let ts_ms = operation.ts_ms;
    write_telemetry_batch(&dir, [TelemetryRecord::Operation(operation)].into_iter());

    let mut summary = Summary::default();
    for_each_stored(&dir, ts_ms, ts_ms + 1, |event| summary.add(&event));
    summary.response("test-node", 1, 0, Vec::new(), None)
}

#[test]
fn metrics_review_persisted_exec_matches_in_memory_command_group() {
    let mut event = Event::blank("exec");
    event.ts_ms = now_ms();
    event.agent = "test-agent".to_string();
    event.host = "test-host".to_string();
    event.workspace = "test-workspace".to_string();
    event.command = "cargo check --all-targets".to_string();
    event.method = command_method(&event.command);
    event.duration_ms = 42;

    let mut memory_summary = Summary::default();
    memory_summary.add(&event);
    let memory = memory_summary.response("test-node", 1, 1, Vec::new(), None);
    let persisted = summarize_persisted(event.to_operation_metric("test-node"));

    assert_eq!(memory.execs.len(), 1);
    assert_eq!(persisted.execs.len(), 1);
    assert_eq!(memory.execs[0].command, "cargo-check");
    assert_eq!(persisted.execs[0].command, memory.execs[0].command);
    assert_eq!(persisted.execs[0].count, memory.execs[0].count);
}

#[test]
fn metrics_review_unknown_persisted_categories_are_ignored() {
    for category in ["cluster", "metrics"] {
        let response = summarize_persisted(operation(now_ms(), category, "request"));
        assert!(
            response.queries.is_empty(),
            "unknown {category} operation must not become an LSP query"
        );
        assert!(response.execs.is_empty());
        assert_eq!(response.sync_rounds, 0);
    }
}

/// A historical scan does not hold the ring: while the scan is kept on an open daily file, an
/// event is recorded from another thread (#1416).
#[test]
fn recording_does_not_wait_for_a_historical_scan() {
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    let temp = tempfile::tempdir().expect("metrics temp dir");
    let dir = temp.path().join("metrics");
    std::fs::create_dir_all(&dir).expect("metrics dir");
    let day_ago = now_ms() - 24 * 3600 * 1000;
    write_telemetry_batch(
        &dir,
        [TelemetryRecord::Operation(operation(
            day_ago, "lsp", "hover",
        ))]
        .into_iter(),
    );
    let metrics = Arc::new(super::Metrics::new(dir.clone()));
    metrics.record(Event::blank("lsp"));

    let (entered_tx, entered) = mpsc::channel();
    let (release, released) = mpsc::channel::<()>();
    let released = std::sync::Mutex::new(released);
    super::storage::scan_hook::set(&dir, move || {
        let _ = entered_tx.send(());
        let _ = released
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(10));
    });
    let scanning = Arc::clone(&metrics);
    let scan = std::thread::spawn(move || scanning.summary("test-node", 2 * 24 * 3600));
    entered
        .recv_timeout(Duration::from_secs(10))
        .expect("the scan reaches the daily file");

    let (recorded_tx, recorded) = mpsc::channel();
    let recording = Arc::clone(&metrics);
    std::thread::spawn(move || {
        recording.record(Event::blank("lsp"));
        let _ = recorded_tx.send(());
    });
    let during_scan = recorded.recv_timeout(Duration::from_secs(2));
    let _ = release.send(());
    let summary = scan.join().expect("the scan finishes");
    super::storage::scan_hook::clear();

    assert!(
        during_scan.is_ok(),
        "recording waited for the historical scan"
    );
    assert_eq!(summary.queries.iter().map(|q| q.count).sum::<u64>(), 2);
}
