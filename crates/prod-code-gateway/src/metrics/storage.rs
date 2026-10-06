/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! JSONL disk persistence and historical metrics queries.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use super::event::{Event, Stored};

/// Calls `f` with every event in the daily files of `dir` from `from_ms` (inclusive) to
/// `to_ms` (exclusive), a line at a time, so a week of events is never held at once.
pub(crate) fn for_each_stored(dir: &Path, from_ms: u64, to_ms: u64, mut f: impl FnMut(Event)) {
    use std::io::BufRead;
    for day in days_since_epoch(from_ms)..=days_since_epoch(to_ms) {
        let path = dir.join(format!("events-{}.jsonl", format_day(day)));
        let Ok(file) = std::fs::File::open(&path) else {
            continue;
        };
        for line in std::io::BufReader::new(file).lines().map_while(Result::ok) {
            let Ok(stored) = serde_json::from_str::<Stored>(&line) else {
                continue;
            };
            if stored.ts_ms < from_ms || stored.ts_ms >= to_ms {
                continue;
            }
            let kind = match stored.kind.as_str() {
                "lsp" => "lsp",
                "exec" => "exec",
                "sync" => "sync",
                _ => continue,
            };
            let mut ev = Event::blank(kind);
            ev.ts_ms = stored.ts_ms;
            ev.agent = stored.agent;
            ev.host = stored.host;
            ev.workspace = stored.workspace;
            ev.method = stored.method;
            ev.duration_ms = stored.duration_ms;
            ev.ok = stored.ok;
            ev.items = stored.items;
            ev.command = stored.command;
            ev.bytes = stored.bytes;
            f(ev);
        }
    }
}

/// Drains queued events to their daily JSONL files in batches until every sender is gone.
pub async fn run_writer(dir: PathBuf, mut rx: rapidfire::mpsc::Receiver<Event>) {
    let mut batch: Vec<Event> = Vec::with_capacity(256);
    while rx.recv_many(&mut batch, 256).await.is_ok() {
        write_events(&dir, batch.drain(..));
    }
}

/// Appends events to their daily files (one open per day per batch).
pub fn write_events(dir: &Path, events: impl Iterator<Item = Event>) {
    let mut files: BTreeMap<String, std::fs::File> = BTreeMap::new();
    for event in events {
        let Ok(line) = serde_json::to_string(&event) else {
            continue;
        };
        let name = format!("events-{}.jsonl", format_day(days_since_epoch(event.ts_ms)));
        let file = match files.entry(name.clone()) {
            std::collections::btree_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::btree_map::Entry::Vacant(v) => {
                match std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(dir.join(&name))
                {
                    Ok(f) => v.insert(f),
                    Err(_) => continue,
                }
            }
        };
        let _ = writeln!(file, "{line}");
    }
}

pub(crate) fn days_since_epoch(ts_ms: u64) -> u64 {
    ts_ms / 86_400_000
}

/// `YYYY-MM-DD` for a day count since the Unix epoch (civil-from-days, Howard Hinnant).
pub(crate) fn format_day(days: u64) -> String {
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}
