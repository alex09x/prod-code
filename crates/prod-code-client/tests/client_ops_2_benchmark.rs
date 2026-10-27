/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_client::divergent_bench::{self, DivergentBenchConfig, WorkspaceMode, WorktreeKind};
use prod_code_testkit::{ScriptedGateway, answers};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Mutex;

#[tokio::test]
async fn persistent_workers_account_for_churn_and_failed_queries() {
    for fail in [false, true] {
        let opened = Mutex::new(HashMap::<String, String>::new());
        let gateway = ScriptedGateway::start(move |method, params| {
            let mut opened = opened.lock().unwrap();
            let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
            match method {
                "textDocument/didOpen" => {
                    opened.insert(
                        uri.into(),
                        params["textDocument"]["text"].as_str().unwrap().into(),
                    );
                    Value::Null
                }
                "textDocument/hover" if fail => answers::failure("fixture hover refused"),
                "textDocument/hover" => json!({"contents":{"value":opened.get(uri).unwrap()}}),
                _ => Value::Null,
            }
        })
        .await;
        let report = divergent_bench::run(DivergentBenchConfig {
            remote: gateway.addr(),
            workers: 12,
            queries_per_worker: 2,
            mode: WorkspaceMode::Shared,
            persistent: true,
            churn_percent: 100,
            ..Default::default()
        })
        .await
        .unwrap();
        assert_eq!(report.total_queries, 24);
        assert_eq!(report.total_errors, if fail { 24 } else { 0 });
        assert_eq!(report.sessions_dropped, 24);
        assert_eq!(report.gateway_after, Some((true, 0)));
        assert_eq!(report.all_passed, !fail);
        assert_eq!(report.latency.count, if fail { 0 } else { 24 });
        assert_eq!(report.initial_syncs.len(), 1);
        for kind in WorktreeKind::all() {
            assert_eq!(
                report.latency_by_kind[&kind].count,
                if fail { 0 } else { 6 }
            );
            if fail {
                assert_eq!(report.errors_by_kind[&kind].0, 6);
                let symbol = if kind == WorktreeKind::UntrackedFile {
                    "divergent_untracked_symbol"
                } else {
                    "compute_signal"
                };
                assert_eq!(
                    report.errors_by_kind[&kind].1,
                    format!(
                        "hover request failed for {symbol}: {{\"code\":-32603,\"message\":\"fixture hover refused\"}}"
                    )
                );
            }
        }
        // When every query fails, each of the four copies also fails on its own (#1522).
        assert_eq!(report.verifications.len(), if fail { 8 } else { 4 });
        assert!(report.verifications.iter().all(|v| v.passed != fail));
        report.print();
    }
}

#[tokio::test]
async fn runner_refuses_zero_queries_and_an_unusable_workdir_before_connecting() {
    let error = divergent_bench::run(DivergentBenchConfig {
        queries_per_worker: 0,
        ..Default::default()
    })
    .await
    .unwrap_err();
    assert_eq!(error.to_string(), "queries_per_worker must be at least 1");
    let file = tempfile::NamedTempFile::new().unwrap();
    let error = divergent_bench::run(DivergentBenchConfig {
        workdir: Some(file.path().to_path_buf()),
        ..Default::default()
    })
    .await
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("failed to create workdir {:?}", file.path())
    );
}
