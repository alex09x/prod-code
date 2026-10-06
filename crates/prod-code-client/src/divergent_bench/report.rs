/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::{
    DivergentTarget, Language, SyncSummary, VerificationResult, WorkspaceMode, WorktreeKind,
};
use crate::LatencyStats;
use std::collections::BTreeMap;
use std::time::Duration;

pub(crate) fn excerpt(text: &str) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > 160 {
        let cut: String = flat.chars().take(160).collect();
        format!("{cut}…")
    } else {
        flat
    }
}

/// Full benchmark report: latency percentiles, throughput, and correctness verification.
#[derive(Debug, Clone)]
pub struct DivergentBenchReport {
    pub language: Language,
    pub mode: WorkspaceMode,
    pub persistent: bool,
    pub churn_percent: u8,
    /// Sessions the workers dropped without a goodbye (churn mode).
    pub sessions_dropped: usize,
    /// Gateway status after the run when churn was on: healthy, and active sessions.
    pub gateway_after: Option<(bool, usize)>,
    pub workspace_name: String,
    pub target: DivergentTarget,
    pub initial_syncs: Vec<SyncSummary>,
    pub total_queries: usize,
    pub total_errors: usize,
    pub elapsed: Duration,
    pub qps: f64,
    pub latency: LatencyStats,
    pub latency_by_kind: BTreeMap<WorktreeKind, LatencyStats>,
    /// Failed query count and first error message per worktree kind.
    pub errors_by_kind: BTreeMap<WorktreeKind, (usize, String)>,
    pub verifications: Vec<VerificationResult>,
    pub all_passed: bool,
}

impl DivergentBenchReport {
    pub fn print(&self) {
        println!("\n📊 Divergent Worktree Benchmark Results:");
        println!("────────────────────────────────────────────────────────────────");
        println!("Language:            {}", self.language.label());
        println!("Workspace Mode:      {}", self.mode.label());
        println!(
            "Session Model:       {}",
            if self.persistent {
                "persistent (one session per worker)"
            } else {
                "connect per query"
            }
        );
        println!("Server Workspace:    {}", self.workspace_name);
        println!(
            "Target Symbol:       {} ({}:{})",
            self.target.symbol,
            self.target.file_rel.display(),
            self.target.line + 1
        );
        for sync in &self.initial_syncs {
            println!(
                "Initial Sync:        {} files, {:.1} KB in {:.0} ms -> {}",
                sync.files,
                sync.bytes as f64 / 1024.0,
                sync.duration.as_secs_f64() * 1000.0,
                sync.workspace_name
            );
        }
        println!("────────────────────────────────────────────────────────────────");
        if self.churn_percent > 0 {
            println!(
                "Session Churn:       {}% ({} session(s) dropped mid-run)",
                self.churn_percent, self.sessions_dropped
            );
            match self.gateway_after {
                Some((healthy, sessions)) => println!(
                    "Gateway After Run:   {}, active sessions {}",
                    if healthy { "HEALTHY" } else { "UNHEALTHY" },
                    sessions
                ),
                None => println!("Gateway After Run:   unreachable"),
            }
        }
        println!("Elapsed Time:        {:.2}s", self.elapsed.as_secs_f64());
        println!("Completed Queries:   {}", self.total_queries);
        println!("Errors:              {}", self.total_errors);
        println!("Throughput:          {:.1} QPS", self.qps);
        println!("Latency (min):       {:.2} ms", self.latency.min_ms);
        println!("Latency (p50):       {:.2} ms", self.latency.p50_ms);
        println!("Latency (p95):       {:.2} ms", self.latency.p95_ms);
        println!("Latency (p99):       {:.2} ms", self.latency.p99_ms);
        println!("Latency (max):       {:.2} ms", self.latency.max_ms);
        for (kind, stats) in &self.latency_by_kind {
            println!(
                "  {:<22} n={:<4} p50={:.2} ms  p95={:.2} ms  p99={:.2} ms",
                kind.label(),
                stats.count,
                stats.p50_ms,
                stats.p95_ms,
                stats.p99_ms
            );
        }
        for (kind, (count, first)) in &self.errors_by_kind {
            println!(
                "  {:<22} errors={:<3} first: {}",
                kind.label(),
                count,
                excerpt(first)
            );
        }
        println!("────────────────────────────────────────────────────────────────");
        println!("Correctness Verification (zero cross-worktree bleed):");
        for v in &self.verifications {
            let mark = if v.passed { "✅ PASS" } else { "❌ FAIL" };
            println!("  {mark}  [{}] {}", v.kind.label(), v.message);
            if !v.passed {
                println!(
                    "          violations={} sample: {}",
                    v.violations,
                    excerpt(&v.sample)
                );
            }
        }
        println!("────────────────────────────────────────────────────────────────");
        println!(
            "Overall Status:      {}",
            if self.all_passed { "PASS" } else { "FAIL" }
        );
    }
}
