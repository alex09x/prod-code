/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Aggregation of metric events into client summary responses.

use prod_code_protocol::{ExecMetric, MetricsResponse, QueryMetric};
use std::collections::BTreeMap;

use super::event::Event;

pub(crate) type Key = (String, String, String, String);

/// Events summed by agent, host, workspace and method (or command).
#[derive(Default)]
pub(crate) struct Summary {
    pub(crate) queries: BTreeMap<Key, Vec<(u64, bool)>>,
    pub(crate) execs: BTreeMap<Key, (u64, u64, u64)>,
    pub(crate) sync_rounds: u64,
    pub(crate) sync_files: u64,
    pub(crate) sync_bytes: u64,
}

impl Summary {
    pub(crate) fn add(&mut self, ev: &Event) {
        let Summary {
            queries,
            execs,
            sync_rounds,
            sync_files,
            sync_bytes,
        } = self;
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
                *sync_rounds += 1;
                *sync_files += ev.items;
                *sync_bytes += ev.bytes;
            }
            _ => {}
        }
    }

    pub(crate) fn response(
        self,
        node: &str,
        since_secs: u64,
        events_in_memory: u64,
    ) -> MetricsResponse {
        let Summary {
            queries,
            execs,
            sync_rounds,
            sync_files,
            sync_bytes,
        } = self;
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
            events_in_memory,
            queries,
            execs,
            sync_rounds,
            sync_files,
            sync_bytes,
        }
    }
}
