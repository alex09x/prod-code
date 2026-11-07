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

use super::event::{Event, command_method};

pub(crate) type Key = (String, String, String, String);

/// One answered query: how long it took, whether it succeeded, and whether it was an answer from
/// an index given while the engine was not ready, and empty (#1975).
#[derive(Clone, Copy)]
pub(crate) struct Sample {
    pub(crate) ms: u64,
    pub(crate) ok: bool,
    pub(crate) before_ready: bool,
    pub(crate) empty: bool,
}

/// Events summed by agent, host, workspace and method (or command).
#[derive(Default)]
pub(crate) struct Summary {
    pub(crate) queries: BTreeMap<Key, Vec<Sample>>,
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
                .push(Sample {
                    ms: ev.duration_ms,
                    ok: ev.ok,
                    // `ready`, or no state at all (a request not answered from an index), is not
                    // an answer given before the engine was ready.
                    before_ready: ev
                        .engine_state
                        .as_deref()
                        .is_some_and(|state| state != "ready"),
                    empty: ev.items == 0,
                }),
            "exec" => {
                let command = if ev.method.is_empty() {
                    command_method(&ev.command)
                } else {
                    ev.method.clone()
                };
                let e = execs
                    .entry((
                        ev.agent.clone(),
                        ev.host.clone(),
                        ev.workspace.clone(),
                        command,
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
        snapshots: Vec<prod_code_protocol::HostSnapshot>,
        inventory: Option<prod_code_protocol::ToolchainInventory>,
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
                samples.sort_by_key(|sample| sample.ms);
                let pct = |p: f64| -> u64 {
                    if samples.is_empty() {
                        0
                    } else {
                        let idx = ((samples.len() as f64 - 1.0) * p).round() as usize;
                        samples[idx.min(samples.len() - 1)].ms
                    }
                };
                QueryMetric {
                    agent,
                    host,
                    workspace,
                    method,
                    count: samples.len() as u64,
                    errors: samples.iter().filter(|sample| !sample.ok).count() as u64,
                    p50_ms: pct(0.5),
                    p95_ms: pct(0.95),
                    max_ms: samples.last().map(|sample| sample.ms).unwrap_or(0),
                    before_ready: samples.iter().filter(|sample| sample.before_ready).count()
                        as u64,
                    empty_before_ready: samples
                        .iter()
                        .filter(|sample| sample.before_ready && sample.empty)
                        .count() as u64,
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
            snapshots,
            inventory,
        }
    }
}
