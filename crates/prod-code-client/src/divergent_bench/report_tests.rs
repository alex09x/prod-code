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

fn report() -> DivergentBenchReport {
    DivergentBenchReport {
        language: Language::Rust,
        mode: WorkspaceMode::Shared,
        persistent: false,
        churn_percent: 0,
        sessions_dropped: 0,
        gateway_after: None,
        workspace_name: "fixture".into(),
        target: DivergentTarget {
            language: Language::Rust,
            file_rel: "src/lib.rs".into(),
            symbol: "sample".into(),
            line: 2,
        },
        initial_syncs: vec![SyncSummary {
            workspace_name: "fixture".into(),
            files: 2,
            bytes: 2048,
            duration: Duration::from_millis(5),
        }],
        total_queries: 4,
        total_errors: 0,
        elapsed: Duration::from_secs(2),
        qps: 2.0,
        latency: LatencyStats::from_micros(&mut [1000, 2000, 3000, 4000]),
        latency_by_kind: BTreeMap::new(),
        errors_by_kind: BTreeMap::new(),
        verifications: vec![],
        all_passed: true,
    }
}

#[test]
fn report_renders_exact_summary() {
    let rendered = report().render();
    let rule = "────────────────────────────────────────────────────────────────";
    assert_eq!(
        rendered,
        format!(
            "\n📊 Divergent Worktree Benchmark Results:\n{rule}\nLanguage:            rust\nWorkspace Mode:      shared\nSession Model:       connect per query\nServer Workspace:    fixture\nTarget Symbol:       sample (src/lib.rs:3)\nInitial Sync:        2 files, 2.0 KB in 5 ms -> fixture\n{rule}\nElapsed Time:        2.00s\nCompleted Queries:   4\nErrors:              0\nThroughput:          2.0 QPS\nLatency (min):       1.00 ms\nLatency (p50):       2.00 ms\nLatency (p95):       4.00 ms\nLatency (p99):       4.00 ms\nLatency (max):       4.00 ms\n{rule}\nCorrectness Verification (zero cross-worktree bleed):\n{rule}\nOverall Status:      PASS\n"
        )
    );
}

#[test]
fn report_renders_churn_errors_and_verification_details() {
    let mut report = report();
    report.persistent = true;
    report.churn_percent = 50;
    report.sessions_dropped = 2;
    report.all_passed = false;
    report
        .latency_by_kind
        .insert(WorktreeKind::Master, report.latency);
    report
        .errors_by_kind
        .insert(WorktreeKind::Master, (3, "bad\n reply".into()));
    report.verifications = vec![
        VerificationResult {
            kind: WorktreeKind::Master,
            passed: true,
            message: "clean".into(),
            violations: 0,
            sample: String::new(),
        },
        VerificationResult {
            kind: WorktreeKind::SignatureChange,
            passed: false,
            message: "bleed".into(),
            violations: 2,
            sample: "bad\n sample".into(),
        },
    ];
    for (status, expected) in [
        (None, "Gateway After Run:   unreachable"),
        (
            Some((true, 0)),
            "Gateway After Run:   HEALTHY, active sessions 0",
        ),
        (
            Some((false, 3)),
            "Gateway After Run:   UNHEALTHY, active sessions 3",
        ),
    ] {
        report.gateway_after = status;
        let rendered = report.render();
        for line in [
            expected,
            "Session Model:       persistent (one session per worker)",
            "Session Churn:       50% (2 session(s) dropped mid-run)",
            "  master                 n=4    p50=2.00 ms  p95=4.00 ms  p99=4.00 ms",
            "  master                 errors=3   first: bad reply",
            "  ✅ PASS  [master] clean",
            "  ❌ FAIL  [worktree-a-signature] bleed",
            "          violations=2 sample: bad sample",
            "Overall Status:      FAIL",
        ] {
            assert!(
                rendered.lines().any(|actual| actual == line),
                "missing {line:?} in {rendered}"
            );
        }
    }
}

#[test]
fn excerpt_collapses_whitespace_and_limits_unicode_characters() {
    for (input, expected) in [("", ""), (" \r\n\t", ""), (" a\tβ\r\n c ", "a β c")] {
        assert_eq!(excerpt(input), expected);
    }
    assert_eq!(excerpt(&"界".repeat(160)), "界".repeat(160));
    assert_eq!(excerpt(&"界".repeat(161)), format!("{}…", "界".repeat(160)));
}
