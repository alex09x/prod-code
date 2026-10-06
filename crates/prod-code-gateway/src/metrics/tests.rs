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
use super::storage::{format_day, write_events};
use super::{Metrics, now_ms};

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
        [hover(now - 2 * day, 10), hover(now - 10 * day, 20)].into_iter(),
    );
    drop(before);
    // After it: one event in memory, which the writer has also put in today's file.
    let after = Metrics::new(dir.clone());
    let recent = hover(now, 30);
    after.record(recent.clone());
    write_events(after.dir(), [recent].into_iter());

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
    while let Ok(ev) = rx.try_recv() {
        pending.push(ev);
    }
    assert_eq!(pending.len(), 6);
    write_events(m.dir(), pending.into_iter());
    let files: Vec<_> = std::fs::read_dir(temp.path().join("metrics"))
        .unwrap()
        .collect();
    assert_eq!(files.len(), 1);
}
