//! Usage metrics: every LSP query, exec and sync round is appended as one JSON line to
//! `<storage>/../metrics/events-YYYY-MM-DD.jsonl` (long-term, importable into ClickHouse) and
//! kept in a bounded in-memory ring for `MetricsRequest` summaries.

use prod_code_protocol::{ExecMetric, MetricsResponse, QueryMetric};
use serde::Serialize;
use std::collections::{BTreeMap, VecDeque};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Debug, Clone, Serialize)]
pub struct Event {
    pub ts_ms: u64,
    /// `lsp`, `exec` or `sync`.
    pub kind: &'static str,
    pub session_id: u64,
    pub client_name: String,
    pub agent: String,
    pub host: String,
    pub client_addr: String,
    pub workspace: String,
    pub engine: String,
    pub method: String,
    pub file: String,
    pub line: u32,
    pub col: u32,
    pub duration_ms: u64,
    pub ok: bool,
    pub items: u64,
    pub command: String,
    pub exit_code: Option<i32>,
    pub bytes: u64,
}

impl Event {
    pub fn blank(kind: &'static str) -> Self {
        Self {
            ts_ms: now_ms(),
            kind,
            session_id: 0,
            client_name: String::new(),
            agent: "unknown".to_string(),
            host: "unknown".to_string(),
            client_addr: String::new(),
            workspace: String::new(),
            engine: String::new(),
            method: String::new(),
            file: String::new(),
            line: 0,
            col: 0,
            duration_ms: 0,
            ok: true,
            items: 0,
            command: String::new(),
            exit_code: None,
            bytes: 0,
        }
    }
}

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
}

impl Metrics {
    pub fn new(dir: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&dir);
        Self {
            dir,
            ring: Mutex::new(VecDeque::with_capacity(4096)),
        }
    }

    /// Records one event: appended to today's JSONL file and to the ring.
    pub fn record(&self, event: Event) {
        if let Ok(line) = serde_json::to_string(&event) {
            let day = days_since_epoch(event.ts_ms);
            let path = self.dir.join(format!("events-{}.jsonl", format_day(day)));
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
            {
                let _ = writeln!(file, "{line}");
            }
        }
        let mut ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        if ring.len() >= RING_CAP {
            ring.pop_front();
        }
        ring.push_back(event);
    }

    /// Aggregates the ring over the last `since_secs` (0 = all).
    pub fn summary(&self, node: &str, since_secs: u64) -> MetricsResponse {
        let ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        let cutoff = if since_secs == 0 {
            0
        } else {
            now_ms().saturating_sub(since_secs * 1000)
        };
        type Key = (String, String, String, String);
        let mut queries: BTreeMap<Key, Vec<(u64, bool)>> = BTreeMap::new();
        let mut execs: BTreeMap<Key, (u64, u64, u64)> = BTreeMap::new();
        let (mut sync_rounds, mut sync_files, mut sync_bytes) = (0, 0, 0);
        for ev in ring.iter().filter(|e| e.ts_ms >= cutoff) {
            match ev.kind {
                "lsp" => queries
                    .entry((
                        ev.agent.clone(),
                        ev.host.clone(),
                        ev.workspace.clone(),
                        ev.method.clone(),
                    ))
                    .or_default()
                    .push((ev.duration_ms, ev.ok)),
                "exec" => {
                    let e = execs
                        .entry((
                            ev.agent.clone(),
                            ev.host.clone(),
                            ev.workspace.clone(),
                            ev.command.clone(),
                        ))
                        .or_default();
                    e.0 += 1;
                    if !ev.ok {
                        e.1 += 1;
                    }
                    e.2 += ev.duration_ms;
                }
                "sync" => {
                    sync_rounds += 1;
                    sync_files += ev.items;
                    sync_bytes += ev.bytes;
                }
                _ => {}
            }
        }
        let queries = queries
            .into_iter()
            .map(|((agent, host, workspace, method), mut samples)| {
                samples.sort_by_key(|(d, _)| *d);
                let pct = |p: f64| -> u64 {
                    if samples.is_empty() {
                        0
                    } else {
                        let idx = ((samples.len() as f64 - 1.0) * p).round() as usize;
                        samples[idx.min(samples.len() - 1)].0
                    }
                };
                QueryMetric {
                    agent,
                    host,
                    workspace,
                    method,
                    count: samples.len() as u64,
                    errors: samples.iter().filter(|(_, ok)| !ok).count() as u64,
                    p50_ms: pct(0.5),
                    p95_ms: pct(0.95),
                    max_ms: samples.last().map(|(d, _)| *d).unwrap_or(0),
                }
            })
            .collect();
        let execs = execs
            .into_iter()
            .map(
                |((agent, host, workspace, command), (count, failures, total_ms))| ExecMetric {
                    agent,
                    host,
                    workspace,
                    command,
                    count,
                    failures,
                    total_ms,
                },
            )
            .collect();
        MetricsResponse {
            node: node.to_string(),
            since_secs,
            events_in_memory: ring.len() as u64,
            queries,
            execs,
            sync_rounds,
            sync_files,
            sync_bytes,
        }
    }
}

fn days_since_epoch(ts_ms: u64) -> u64 {
    ts_ms / 86_400_000
}

/// `YYYY-MM-DD` for a day count since the Unix epoch (civil-from-days, Howard Hinnant).
fn format_day(days: u64) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_format_as_dates() {
        assert_eq!(format_day(0), "1970-01-01");
        assert_eq!(format_day(20_716), "2026-09-20");
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
        let files: Vec<_> = std::fs::read_dir(temp.path().join("metrics"))
            .unwrap()
            .collect();
        assert_eq!(files.len(), 1);
    }
}
