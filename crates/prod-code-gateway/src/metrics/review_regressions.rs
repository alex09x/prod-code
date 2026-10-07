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
