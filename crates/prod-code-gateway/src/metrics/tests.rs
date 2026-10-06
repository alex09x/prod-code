/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::event::Event;
use super::storage::{format_day, prune_expired_metrics, write_events, write_telemetry_batch};
use super::{Metrics, now_ms};
use prod_code_protocol::{
    EngineToolchainInfo, HostSnapshot, OperationMetric, TelemetryRecord, ToolchainInventory,
    ToolchainVersion,
};

#[test]
fn days_format_as_dates() {
    assert_eq!(format_day(0), "1970-01-01");
    assert_eq!(format_day(20_716), "2026-09-20");
}

/// A restart empties the ring, not the files: a window that reaches back before the oldest
/// event in memory is summed from them, and an event both hold counts once (#305).
#[test]
fn a_summary_reaches_past_a_restart_into_the_daily_files() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("metrics");
    let hover = |ts_ms: u64, duration_ms: u64| {
        let mut e = Event::blank("lsp");
        e.ts_ms = ts_ms;
        e.agent = "codex".into();
        e.workspace = "ws".into();
        e.method = "textDocument/hover".into();
        e.duration_ms = duration_ms;
        e
    };
    let day = 86_400_000;
    let now = now_ms();
    // Before the restart: two days ago, and a week and a half ago.
    let before = Metrics::new(dir.clone());
    write_events(
        before.dir(),
        "n",
        [hover(now - 2 * day, 10), hover(now - 10 * day, 20)].into_iter(),
    );
    drop(before);
    // After it: one event in memory, which the writer has also put in today's file.
    let after = Metrics::new(dir.clone());
    let recent = hover(now, 30);
    after.record(recent.clone());
    write_events(after.dir(), "n", [recent].into_iter());

    let week = after.summary("n", 7 * 86_400);
    assert_eq!(week.queries.len(), 1);
    assert_eq!(
        week.queries[0].count, 2,
        "two days ago and now, the latter once"
    );
    assert_eq!(week.queries[0].max_ms, 30);
    assert_eq!(week.events_in_memory, 1);
    let hour = after.summary("n", 3600);
    assert_eq!(hour.queries[0].count, 1);
    let all = after.summary("n", 0);
    assert_eq!(all.queries[0].count, 1, "0 is what memory holds");
    let month = after.summary("n", 30 * 86_400);
    assert_eq!(month.queries[0].count, 3);
}

#[test]
fn summary_groups_and_percentiles() {
    let temp = tempfile::tempdir().unwrap();
    let m = Metrics::new(temp.path().join("metrics"));
    for d in [10, 20, 30, 40, 1000] {
        let mut e = Event::blank("lsp");
        e.agent = "claude-code".into();
        e.workspace = "ws".into();
        e.method = "textDocument/hover".into();
        e.duration_ms = d;
        e.ok = d != 1000;
        m.record(e);
    }
    let mut x = Event::blank("exec");
    x.command = "cargo test".into();
    x.ok = false;
    x.duration_ms = 500;
    m.record(x);
    let s = m.summary("n", 0);
    assert_eq!(s.queries.len(), 1);
    assert_eq!(s.queries[0].count, 5);
    assert_eq!(s.queries[0].errors, 1);
    assert_eq!(s.queries[0].p50_ms, 30);
    assert_eq!(s.queries[0].max_ms, 1000);
    assert_eq!(s.execs[0].failures, 1);
    // The writer task drains the queue to disk; here we drain it by hand.
    let mut rx = m.take_receiver().unwrap();
    let mut pending = Vec::new();
    while let Ok(rec) = rx.try_recv() {
        pending.push(rec);
    }
    assert_eq!(pending.len(), 6);
    write_telemetry_batch(m.dir(), pending.into_iter());
    let files: Vec<_> = std::fs::read_dir(temp.path().join("metrics"))
        .unwrap()
        .collect();
    assert_eq!(files.len(), 1);
}

#[test]
fn records_and_retrieves_snapshots_and_inventory() {
    let temp = tempfile::tempdir().unwrap();
    let m = Metrics::new(temp.path().join("metrics"));

    let snapshot = HostSnapshot {
        ts_ms: now_ms(),
        node: "node-1".into(),
        version: "0.3.26".into(),
        git_commit: "abcd123".into(),
        platform: "linux x86_64".into(),
        cpu_count: 8,
        cpu_usage_millis: Some(15000), // 15.0%
        load_average_millis: Some(1200),
        process_rss_bytes: Some(100 * 1024 * 1024),
        host_memory_available_bytes: Some(8 * 1024 * 1024 * 1024),
        host_memory_total_bytes: Some(16 * 1024 * 1024 * 1024),
        storage_free_millis: Some(400),
        storage_free_bytes: Some(50 * 1024 * 1024 * 1024),
        active_sessions: 3,
        active_queries: 1,
        running_commands: 0,
        workspace_count: 2,
        engine_count: 5,
    };
    m.record_snapshot(snapshot.clone());

    let latest = m.latest_snapshot().expect("has snapshot");
    assert_eq!(latest.node, "node-1");
    assert_eq!(latest.cpu_usage_pct(), Some(15.0));
    assert_eq!(latest.workspace_count, 2);

    let inventory = ToolchainInventory {
        ts_ms: now_ms(),
        node: "node-1".into(),
        engines: vec![EngineToolchainInfo {
            engine: "rust".into(),
            available: true,
            toolchains: vec![ToolchainVersion {
                tool: "rustc".into(),
                version: "1.97.1".into(),
            }],
            details: None,
        }],
    };
    m.record_inventory(inventory.clone());

    let cached_inv = m.toolchain_inventory().expect("has inventory");
    assert_eq!(cached_inv.engines.len(), 1);
    assert_eq!(cached_inv.engines[0].engine, "rust");

    let sum = m.summary("node-1", 0);
    assert_eq!(sum.snapshots.len(), 1);
    assert!(sum.inventory.is_some());
}

#[test]
fn telemetry_record_json_serialization_round_trips() {
    let op = OperationMetric::new("node-a", "search", "search_query");
    let rec = TelemetryRecord::Operation(op);
    let json = serde_json::to_string(&rec).unwrap();
    assert!(json.contains("\"type\":\"operation\""));
    let parsed: TelemetryRecord = serde_json::from_str(&json).unwrap();
    assert_eq!(rec, parsed);
}

#[test]
fn retention_prunes_stale_files_and_keeps_capacity() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("metrics");
    std::fs::create_dir_all(&dir).unwrap();

    let pruned = prune_expired_metrics(&dir, 14, 100 * 1024 * 1024);
    assert_eq!(pruned, 0);
}

#[test]
fn prometheus_formatting_outputs_valid_exposition_text() {
    let temp = tempfile::tempdir().unwrap();
    let m = Metrics::new(temp.path().join("metrics"));

    let mut ev = Event::blank("lsp");
    ev.method = "textDocument/definition".into();
    ev.engine = "rust".into();
    ev.duration_ms = 45;
    ev.items = 1;
    m.record(ev);

    let snap = HostSnapshot {
        ts_ms: now_ms(),
        node: "node-x".into(),
        version: "0.3.26".into(),
        git_commit: "deadbeef".into(),
        platform: "linux x86_64".into(),
        cpu_count: 8,
        cpu_usage_millis: Some(12500), // 12.5%
        load_average_millis: Some(850),
        process_rss_bytes: Some(50 * 1024 * 1024),
        host_memory_available_bytes: Some(16 * 1024 * 1024 * 1024),
        host_memory_total_bytes: Some(32 * 1024 * 1024 * 1024),
        storage_free_millis: Some(250),
        storage_free_bytes: Some(100 * 1024 * 1024 * 1024),
        active_sessions: 1,
        active_queries: 0,
        running_commands: 0,
        workspace_count: 2,
        engine_count: 3,
    };
    m.record_snapshot(snap);

    let inv = ToolchainInventory {
        ts_ms: now_ms(),
        node: "node-x".into(),
        engines: vec![EngineToolchainInfo {
            engine: "rust".into(),
            available: true,
            toolchains: vec![ToolchainVersion {
                tool: "rustc".into(),
                version: "1.97.1".into(),
            }],
            details: None,
        }],
    };
    m.record_inventory(inv);

    let text = super::format_prometheus_metrics(&m, Some("node-x"));
    assert!(text.contains("# HELP prod_code_operations_total"));
    assert!(text.contains("prod_code_operations_total{node=\"node-x\",category=\"lsp\",method=\"textDocument/definition\",engine=\"rust\",status=\"ok\"} 1"));
    assert!(text.contains("prod_code_gateway_cpu_usage_ratio{node=\"node-x\"} 0.1250"));
    assert!(text.contains("prod_code_gateway_cpu_count{node=\"node-x\"} 8"));
    assert!(text.contains("prod_code_gateway_info{node=\"node-x\",version=\"0.3.26\",git_commit=\"deadbeef\",platform=\"linux x86_64\"} 1"));
    assert!(text.contains("prod_code_toolchain_info{node=\"node-x\",engine=\"rust\",tool=\"rustc\",version=\"1.97.1\"} 1"));
}

#[tokio::test]
async fn prometheus_http_server_serves_metrics_and_health() {
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    let temp = tempfile::tempdir().unwrap();
    let m = Arc::new(Metrics::new(temp.path().join("metrics")));

    // Bind on random ephemeral port
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let server_m = Arc::clone(&m);
    tokio::spawn(async move {
        let _ = super::run_prometheus_server(addr, server_m, "test-host".to_string()).await;
    });

    // Wait a moment for server to bind
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Test GET /metrics
    let mut client = TcpStream::connect(addr).await.expect("connect");
    client
        .write_all(b"GET /metrics HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    let mut resp = String::new();
    client.read_to_string(&mut resp).await.unwrap();
    assert!(resp.starts_with("HTTP/1.1 200 OK"));
    assert!(resp.contains("Content-Type: text/plain; version=0.0.4"));

    // Test GET /health
    let mut client = TcpStream::connect(addr).await.expect("connect");
    client
        .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    let mut resp = String::new();
    client.read_to_string(&mut resp).await.unwrap();
    assert!(resp.starts_with("HTTP/1.1 200 OK"));
    assert!(resp.contains("OK"));

    // Test GET /nonexistent
    let mut client = TcpStream::connect(addr).await.expect("connect");
    client
        .write_all(b"GET /unknown HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    let mut resp = String::new();
    client.read_to_string(&mut resp).await.unwrap();
    assert!(resp.starts_with("HTTP/1.1 404 Not Found"));
}

#[test]
fn prometheus_push_url_builder() {
    let url = super::prometheus_push::build_pushgateway_url(
        "http://push.example.com:9091",
        "prod-code",
        "worker-1:9400",
    )
    .unwrap();
    assert_eq!(
        url.as_str(),
        "http://push.example.com:9091/metrics/job/prod-code/instance/worker-1%3A9400"
    );
}

#[tokio::test]
async fn prometheus_push_to_mock_server() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server_task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 1024];
        let n = stream.read(&mut buf).await.unwrap();
        let req = std::str::from_utf8(&buf[..n]).unwrap();
        assert!(req.starts_with("POST /metrics/job/prod-code/instance/inst-1 HTTP/1.1"));
        assert!(req.contains("my_metric 42"));

        stream
            .write_all(b"HTTP/1.1 202 Accepted\r\n\r\n")
            .await
            .unwrap();
    });

    let base = format!("http://{}", addr);
    super::prometheus_push::push_metrics_to_gateway(&base, "prod-code", "inst-1", "my_metric 42\n")
        .await
        .expect("push succeeds");

    server_task.await.unwrap();
}
