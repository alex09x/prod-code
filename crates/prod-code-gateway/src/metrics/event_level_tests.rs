/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;

#[test]
fn default_level_filters_status_polls_and_writes_real_operations_and_failed_status() {
    let temp = tempfile::tempdir().unwrap();
    let metrics_dir = temp.path().join("metrics");
    let m = Metrics::new(metrics_dir.clone());
    assert_eq!(m.event_level(), MetricsEventLevel::Operations);

    // 1. Successful status poll: should be filtered
    let mut ev_status_ok = Event::blank("status");
    ev_status_ok.method = "status".into();
    ev_status_ok.duration_ms = 2;
    ev_status_ok.ok = true;
    m.record(ev_status_ok);

    // 2. Real operations: sync, lsp, exec
    let mut ev_sync = Event::blank("sync");
    ev_sync.method = "sync".into();
    ev_sync.duration_ms = 25;
    ev_sync.items = 10;
    ev_sync.bytes = 1024;
    m.record(ev_sync);

    let mut ev_lsp = Event::blank("lsp");
    ev_lsp.method = "textDocument/definition".into();
    ev_lsp.engine = "rust".into();
    ev_lsp.duration_ms = 45;
    m.record(ev_lsp);

    let mut ev_exec = Event::blank("exec");
    ev_exec.command = "cargo check".into();
    ev_exec.duration_ms = 1500;
    m.record(ev_exec);

    // 3. Failed status poll: should NOT be filtered
    let mut ev_status_fail = Event::blank("status");
    ev_status_fail.method = "status".into();
    ev_status_fail.duration_ms = 5;
    ev_status_fail.ok = false;
    ev_status_fail.error_class = Some("internal_error".into());
    m.record(ev_status_fail);

    // Verify in-memory ring: holds only the 4 unfiltered events, not the successful status poll
    let mut recorded_kinds = Vec::new();
    m.for_each_ring_event(|e| {
        recorded_kinds.push((e.kind, e.ok));
    });
    assert_eq!(
        recorded_kinds,
        vec![
            ("sync", true),
            ("lsp", true),
            ("exec", true),
            ("status", false),
        ]
    );

    // Verify filtered counter
    assert_eq!(m.status_polls(), 1);
    assert_eq!(m.status_poll_duration_ms(), 2);

    // Verify disk writer writes only the 4 unfiltered events
    let mut rx = m.take_receiver().expect("rx available");
    let mut pending = Vec::new();
    while let Ok(rec) = rx.try_recv() {
        pending.push(rec);
    }
    storage::write_telemetry_batch(&metrics_dir, pending.into_iter());

    // Inspect files written to disk
    let mut disk_records = Vec::new();
    let now = now_ms();
    storage::for_each_stored(
        &metrics_dir,
        now.saturating_sub(60_000),
        now + 60_000,
        |ev| {
            disk_records.push((ev.kind, ev.ok));
        },
    );
    assert_eq!(
        disk_records,
        vec![
            ("sync", true),
            ("lsp", true),
            ("exec", true),
            ("status", false),
        ]
    );

    // Verify Prometheus output: includes both the filtered status poll count and the failed status event
    let prom = format_prometheus_metrics(&m, Some("test-node"));
    assert!(
        prom.contains("prod_code_operations_total{node=\"test-node\",category=\"status\",method=\"status\",engine=\"none\",status=\"ok\"} 1"),
        "Prometheus should include the filtered successful status poll counter:\n{prom}"
    );
    assert!(
        prom.contains("prod_code_operations_total{node=\"test-node\",category=\"status\",method=\"status\",engine=\"none\",status=\"error\",error_class=\"internal_error\"} 1"),
        "Prometheus should include the failed status poll event:\n{prom}"
    );
}

#[test]
fn all_level_records_successful_status_polls_to_ring_and_disk() {
    let temp = tempfile::tempdir().unwrap();
    let metrics_dir = temp.path().join("metrics");
    let m = Metrics::new(metrics_dir.clone());
    m.set_event_level(MetricsEventLevel::All);
    assert_eq!(m.event_level(), MetricsEventLevel::All);

    let mut ev_status_ok = Event::blank("status");
    ev_status_ok.method = "status".into();
    ev_status_ok.duration_ms = 3;
    ev_status_ok.ok = true;
    m.record(ev_status_ok);

    // In 'all' mode, successful status is in the ring
    let mut recorded_kinds = Vec::new();
    m.for_each_ring_event(|e| {
        recorded_kinds.push((e.kind, e.ok));
    });
    assert_eq!(recorded_kinds, vec![("status", true)]);
    assert_eq!(m.status_polls(), 0);

    // And written to disk
    let mut rx = m.take_receiver().expect("rx available");
    let mut pending = Vec::new();
    while let Ok(rec) = rx.try_recv() {
        pending.push(rec);
    }
    storage::write_telemetry_batch(&metrics_dir, pending.into_iter());

    let mut disk_records = Vec::new();
    let now = now_ms();
    storage::for_each_stored(
        &metrics_dir,
        now.saturating_sub(60_000),
        now + 60_000,
        |ev| {
            disk_records.push((ev.kind, ev.ok));
        },
    );
    assert_eq!(disk_records, vec![("status", true)]);

    let prom = format_prometheus_metrics(&m, Some("test-node"));
    assert!(
        prom.contains("prod_code_operations_total{node=\"test-node\",category=\"status\",method=\"status\",engine=\"none\",status=\"ok\"} 1")
    );
}

#[test]
fn config_parsing_and_rejection_of_bad_event_level() {
    // Valid values
    let toml_ops = "[metrics]\nevent_level = \"operations\"\n";
    let cfg_ops: crate::state::config::GatewayConfigFile = toml::from_str(toml_ops).unwrap();
    assert_eq!(
        cfg_ops.metrics.event_level,
        Some(MetricsEventLevel::Operations)
    );

    let toml_all = "[metrics]\nevent_level = \"all\"\n";
    let cfg_all: crate::state::config::GatewayConfigFile = toml::from_str(toml_all).unwrap();
    assert_eq!(cfg_all.metrics.event_level, Some(MetricsEventLevel::All));

    // Bad value rejected with key named and accepted values named
    let toml_bad = "[metrics]\nevent_level = \"verbose\"\n";
    let err = toml::from_str::<crate::state::config::GatewayConfigFile>(toml_bad).unwrap_err();
    let err_str = err.to_string();
    assert!(
        err_str.contains("event_level"),
        "error should name the key 'event_level': {err_str}"
    );
    assert!(
        err_str.contains("'operations' or 'all'"),
        "error should name accepted values: {err_str}"
    );

    // CLI parse bad value rejected
    let cli_err = "verbose".parse::<MetricsEventLevel>().unwrap_err();
    let cli_err_str = cli_err.to_string();
    assert!(cli_err_str.contains("event_level"));
    assert!(cli_err_str.contains("'operations' or 'all'"));
}
