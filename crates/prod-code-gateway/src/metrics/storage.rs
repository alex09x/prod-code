/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! JSONL disk persistence, bounded retention pruning, and historical queries.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use super::event::{Event, Stored};
use prod_code_protocol::TelemetryRecord;

/// Default retention period: 14 days of telemetry before automated pruning.
pub const DEFAULT_METRICS_RETENTION_DAYS: u64 = 14;

/// Maximum disk budget for metric JSONL files (500 MB) before emergency age-tiered pruning.
pub const MAX_METRICS_STORAGE_BYTES: u64 = 500 * 1024 * 1024;

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
            // First try parsing as TelemetryRecord (operation)
            if let Ok(rec) = serde_json::from_str::<TelemetryRecord>(&line) {
                if let TelemetryRecord::Operation(op) = rec {
                    if op.ts_ms < from_ms || op.ts_ms >= to_ms {
                        continue;
                    }
                    let kind = match op.category.as_str() {
                        "lsp" => "lsp",
                        "exec" | "remote_exec" => "exec",
                        "sync" => "sync",
                        "search" => "search",
                        "shadow" => "shadow",
                        "read_file" => "read_file",
                        "place" => "place",
                        "status" => "status",
                        _ => "lsp",
                    };
                    let mut ev = Event::blank(kind);
                    ev.ts_ms = op.ts_ms;
                    ev.host = op.host.unwrap_or(op.node);
                    ev.agent = op.agent.unwrap_or_else(|| "unknown".to_string());
                    ev.workspace = op.workspace.unwrap_or_default();
                    ev.engine = op.engine;
                    ev.method = op.method;
                    ev.duration_ms = op.duration_ms;
                    ev.ok = op.ok;
                    ev.items = op.items;
                    ev.bytes = op.bytes;
                    ev.error_class = op.error_class;
                    ev.exit_code = op.exit_code;
                    f(ev);
                }
                continue;
            }

            // Fallback: parse as legacy Stored
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
                "search" => "search",
                "shadow" => "shadow",
                "read_file" => "read_file",
                "place" => "place",
                "status" => "status",
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

/// Drains queued telemetry records to daily JSONL files in batches until every sender is gone.
pub async fn run_writer(dir: PathBuf, mut rx: rapidfire::mpsc::Receiver<TelemetryRecord>) {
    let mut batch: Vec<TelemetryRecord> = Vec::with_capacity(256);
    while rx.recv_many(&mut batch, 256).await.is_ok() {
        write_telemetry_batch(&dir, batch.drain(..));
    }
}

/// Appends telemetry records to their daily files (one open per day per batch).
pub fn write_telemetry_batch(dir: &Path, records: impl Iterator<Item = TelemetryRecord>) {
    let mut files: BTreeMap<String, std::fs::File> = BTreeMap::new();
    for rec in records {
        let Ok(line) = serde_json::to_string(&rec) else {
            continue;
        };
        let name = format!(
            "events-{}.jsonl",
            format_day(days_since_epoch(rec.timestamp_ms()))
        );
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

/// Legacy write_events helper for Event iterators (wraps into TelemetryRecord).
#[allow(dead_code)]
pub fn write_events(dir: &Path, node: &str, events: impl Iterator<Item = Event>) {
    let records = events.map(|e| TelemetryRecord::Operation(e.to_operation_metric(node)));
    write_telemetry_batch(dir, records);
}

/// Prunes metric files older than `retention_days` and enforces `max_bytes` ceiling.
///
/// Returns the number of files deleted.
pub fn prune_expired_metrics(dir: &Path, retention_days: u64, max_bytes: u64) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };

    let now = super::now_ms();
    let current_day = days_since_epoch(now);
    let cutoff_day = current_day.saturating_sub(retention_days);

    let mut files: Vec<(String, u64, u64, PathBuf)> = Vec::new(); // (name, day, size_bytes, path)
    let mut total_bytes = 0u64;

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("events-") || !name.ends_with(".jsonl") {
            continue;
        }
        let date_part = name
            .trim_start_matches("events-")
            .trim_end_matches(".jsonl");
        let day_opt = parse_day_from_date(date_part);
        let Ok(meta) = entry.metadata() else { continue };
        let size = meta.len();
        total_bytes += size;

        let day = day_opt.unwrap_or(0);
        files.push((name, day, size, entry.path()));
    }

    let mut pruned = 0;

    // 1. Prune by retention days
    files.retain(|(name, day, size, path)| {
        if *day > 0 && *day < cutoff_day {
            if std::fs::remove_file(path).is_ok() {
                pruned += 1;
                total_bytes = total_bytes.saturating_sub(*size);
                tracing::info!(file = %name, "pruned expired telemetry log");
                return false;
            }
        }
        true
    });

    // 2. Prune by capacity budget if total exceeds max_bytes
    if total_bytes > max_bytes {
        files.sort_by_key(|(_, day, _, _)| *day);
        for (name, _, size, path) in files {
            if total_bytes <= max_bytes {
                break;
            }
            if std::fs::remove_file(&path).is_ok() {
                pruned += 1;
                total_bytes = total_bytes.saturating_sub(size);
                tracing::warn!(file = %name, "pruned telemetry log to respect disk budget");
            }
        }
    }

    pruned
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

/// Parses `YYYY-MM-DD` into days since Unix epoch.
pub(crate) fn parse_day_from_date(s: &str) -> Option<u64> {
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 3 {
        return None;
    }
    let y: i64 = parts[0].parse().ok()?;
    let m: i64 = parts[1].parse().ok()?;
    let d: i64 = parts[2].parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    if days >= 0 { Some(days as u64) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_parsing_round_trips() {
        for days in [0, 100, 1000, 20_000, 20_716, 25_000] {
            let formatted = format_day(days);
            let parsed = parse_day_from_date(&formatted).expect("valid parse");
            assert_eq!(parsed, days, "failed for day {days} ({formatted})");
        }
    }

    #[test]
    fn prunes_expired_metrics_files() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();

        // Create an old file (20 days ago) and a fresh file (today)
        let now = super::super::now_ms();
        let today = days_since_epoch(now);
        let old_day = today.saturating_sub(20);

        let old_file = dir.join(format!("events-{}.jsonl", format_day(old_day)));
        let fresh_file = dir.join(format!("events-{}.jsonl", format_day(today)));

        std::fs::write(&old_file, "{\"type\":\"test\"}\n").unwrap();
        std::fs::write(&fresh_file, "{\"type\":\"test\"}\n").unwrap();

        let pruned = prune_expired_metrics(dir, 14, 100 * 1024 * 1024);
        assert_eq!(pruned, 1);
        assert!(!old_file.exists());
        assert!(fresh_file.exists());
    }
}
