/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{ValidationProgress, batch::validate_texts_with_budget, budget::ValidationBudget};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::Instant;

#[test]
fn validate_interactive_budget_is_120_seconds_without_shortening_refactorings() {
    let now = Instant::now();
    let interactive = ValidationBudget::interactive();
    let refactoring = ValidationBudget::new();
    assert!(interactive.deadline >= now + Duration::from_secs(120));
    assert!(interactive.deadline < now + Duration::from_secs(121));
    assert!(refactoring.deadline >= now + Duration::from_secs(600));
    assert!(refactoring.deadline < now + Duration::from_secs(601));
    assert!(interactive.baseline().deadline < now + Duration::from_secs(31));
    assert!(refactoring.baseline().deadline >= now + Duration::from_secs(600));
}

// TABLE: progress must describe the currently blocked phase and retain finished
// reports, for the first/middle/last proposal and an unchanged caller.
// silent file | edited files | caller | expected report codes in request order
// a.rs        | a,b          | c      | incomplete, clean, clean
// b.rs        | a,b          | c      | clean, incomplete, clean
// c.rs        | a,b          | c      | clean, clean, incomplete
#[tokio::test]
async fn validate_progress_preserves_reports_and_counts_callers_table() {
    let ws = Workspace::new(&[
        ("a.rs", "fn a() {}"),
        ("b.rs", "fn b() {}"),
        ("c.rs", "fn c() {}"),
    ]);
    for (index, silent) in ["a.rs", "b.rs", "c.rs"].into_iter().enumerate() {
        let proposed = Arc::new(Mutex::new(false));
        let gateway = ScriptedGateway::start(move |method, params| {
            if method == "textDocument/didOpen"
                && params["textDocument"]["text"]
                    .as_str()
                    .is_some_and(|s| s.contains("proposed"))
            {
                *proposed.lock().unwrap() = true;
            }
            if method == "textDocument/diagnostic" {
                if *proposed.lock().unwrap()
                    && params["textDocument"]["uri"]
                        .as_str()
                        .unwrap()
                        .ends_with(silent)
                {
                    return json!({"prod-code/no-response": true});
                }
                return answers::no_diagnostics();
            }
            json!([])
        })
        .await;
        let progress = ValidationProgress::new(3);
        let budget = ValidationBudget {
            diagnostic: Duration::from_millis(100),
            progress: Some(progress.clone()),
            ..ValidationBudget::interactive()
        };
        let edits = [
            ("a.rs".into(), "fn proposed_a() {}".into()),
            ("b.rs".into(), "fn proposed_b() {}".into()),
        ];
        let root = ws.root();
        let callers = ["c.rs".into()];
        let validation =
            validate_texts_with_budget(gateway.addr(), &root, &edits, &callers, &budget);
        tokio::pin!(validation);
        // Observe the snapshot while the real session is waiting for the missing reply.
        let observed = async {
            loop {
                let state = progress.snapshot();
                if state.0 == "textDocument/diagnostic" && state.1 == index {
                    return state;
                }
                tokio::task::yield_now().await;
            }
        };
        let (reports, state) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(validation, observed)
        })
        .await
        .unwrap();
        assert_eq!(state, ("textDocument/diagnostic".into(), index, 3));
        let reports = reports.unwrap();
        assert_eq!(reports.len(), 3);
        for (i, (report, file)) in reports.iter().zip(["a.rs", "b.rs", "c.rs"]).enumerate() {
            assert_eq!(report.file, file);
            assert_eq!(report.ok(), i != index, "{report:?}");
            if i == index {
                assert_eq!(
                    report.items[0].code.as_deref(),
                    Some("prod-code-validation-incomplete")
                );
                assert!(
                    report.items[0]
                        .message
                        .contains(&format!("{file} was not validated"))
                );
            } else {
                assert!(report.items.is_empty());
            }
        }
        assert_eq!(progress.snapshot().1, 3);
    }
}

#[tokio::test]
async fn validate_progress_counts_parser_files_in_a_mixed_batch() {
    let ws = Workspace::new(&[("a.rs", "fn a() {}")]);
    let gateway = ScriptedGateway::start(|method, _| {
        if method == "textDocument/diagnostic" {
            answers::no_diagnostics()
        } else {
            json!([])
        }
    })
    .await;
    let progress = ValidationProgress::new(2);
    let budget = ValidationBudget {
        progress: Some(progress.clone()),
        ..ValidationBudget::interactive()
    };
    let reports = validate_texts_with_budget(
        gateway.addr(),
        &ws.root(),
        &[
            ("a.rs".into(), "fn a() {}".into()),
            ("data.json".into(), "{}".into()),
        ],
        &[],
        &budget,
    )
    .await
    .unwrap();
    assert_eq!(reports.len(), 2);
    assert!(reports.iter().all(|r| r.ok()), "{reports:?}");
    assert_eq!(progress.snapshot().1, 2);
}
