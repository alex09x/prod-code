/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::json;

use super::validate_texts;

#[test]
fn validate_batch_keeps_its_state_out_of_the_tool_dispatcher_stack() {
    let remote = "127.0.0.1:1".parse().unwrap();
    let validation = validate_texts(remote, std::path::Path::new("."), &[], &[]);
    let bytes = std::mem::size_of_val(&validation);
    assert!(
        bytes <= 1024,
        "batch validation embeds {bytes} bytes in its caller"
    );
}

#[tokio::test]
async fn validate_batch_retries_pending_write_after_all_proposals_are_open() {
    let ws = Workspace::new(&[("a.rs", "fn old_a() {}"), ("b.rs", "fn old_b() {}")]);
    let opened = Arc::new(AtomicUsize::new(0));
    let cancelled = Arc::new(AtomicBool::new(false));
    let gateway = ScriptedGateway::start({
        let opened = Arc::clone(&opened);
        let cancelled = Arc::clone(&cancelled);
        move |method, params| {
            if method == "textDocument/didOpen"
                && params["textDocument"]["text"]
                    .as_str()
                    .is_some_and(|t| t.contains("proposed"))
            {
                opened.fetch_add(1, Ordering::SeqCst);
            }
            if method == "textDocument/diagnostic" {
                if opened.load(Ordering::SeqCst) > 0 {
                    assert_eq!(
                        opened.load(Ordering::SeqCst),
                        2,
                        "the whole overlay precedes diagnostics"
                    );
                    if !cancelled.swap(true, Ordering::SeqCst) {
                        return answers::failure("cancelled because of pending write");
                    }
                }
                return answers::no_diagnostics();
            }
            json!([])
        }
    })
    .await;
    let reports = validate_texts(
        gateway.addr(),
        &ws.root(),
        &[
            ("a.rs".into(), "fn proposed_a() {}".into()),
            ("b.rs".into(), "fn proposed_b() {}".into()),
        ],
        &[],
    )
    .await
    .expect("a cancelled snapshot must be retried");
    assert!(cancelled.load(Ordering::SeqCst));
    assert_eq!(reports.len(), 2);
    assert!(reports.iter().all(|r| r.ok()), "{reports:?}");
}

#[tokio::test]
async fn validate_batch_timeout_preserves_other_files_and_also_check() {
    let ws = Workspace::new(&[
        ("a.rs", "fn a() {}"),
        ("b.rs", "fn b() {}"),
        ("c.rs", "fn c() {}"),
    ]);
    let proposed = Arc::new(AtomicBool::new(false));
    let gateway = ScriptedGateway::start(move |method, params| {
        if method == "textDocument/didOpen"
            && params["textDocument"]["text"]
                .as_str()
                .is_some_and(|t| t.contains("proposed"))
        {
            proposed.store(true, Ordering::SeqCst);
        }
        if method == "textDocument/diagnostic" {
            let uri = params["textDocument"]["uri"].as_str().unwrap();
            if proposed.load(Ordering::SeqCst) {
                if uri.ends_with("/a.rs") || uri.ends_with("/c.rs") {
                    return answers::failure("query timed out after 60s");
                }
                return answers::error_at(1, 1, "E0425", "missing value");
            }
            return answers::no_diagnostics();
        }
        json!([])
    })
    .await;
    let edits = [
        ("a.rs".into(), "fn proposed_a() {}".into()),
        ("b.rs".into(), "fn proposed_b() {}".into()),
    ];
    // A refactoring's own check refuses to call an unchecked file clean or broken.
    let refused = validate_texts(gateway.addr(), &ws.root(), &edits, &["c.rs".into()])
        .await
        .expect_err("an unchecked file stops a refactoring's check");
    assert!(
        refused.to_string().contains("was not validated"),
        "{refused:#}"
    );
    // The agent's own call gets every result it can.
    let reports =
        super::validate_texts_interactive(gateway.addr(), &ws.root(), &edits, &["c.rs".into()])
            .await
            .expect("a slow file must yield a partial batch");
    assert_eq!(
        reports.iter().map(|r| r.file.as_str()).collect::<Vec<_>>(),
        ["a.rs", "b.rs", "c.rs"]
    );
    for i in [0, 2] {
        assert!(!reports[i].ok());
        assert!(reports[i].items[0].message.contains("not validated"));
    }
    assert_eq!(reports[1].items[0].code.as_deref(), Some("E0425"));
}

#[tokio::test]
async fn validate_batch_per_file_deadline_does_not_mistake_a_late_reply_for_the_next_file() {
    use super::budget::ValidationBudget;
    use crate::session::LspSession;
    use futures_util::{SinkExt, StreamExt};
    use prod_code_protocol::{AnyStream, ProdCodeCodec, WireMessage};
    use std::collections::HashMap;
    use std::time::Duration;
    use tokio_util::codec::Framed;

    let ws = Workspace::new(&[("a.rs", "fn a() {}"), ("b.rs", "fn b() {}")]);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let peer = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut framed = Framed::new(socket, ProdCodeCodec::new());
        for index in 0..2 {
            let Some(Ok(WireMessage::LspPayload(request))) = framed.next().await else {
                panic!("expected a diagnostic request")
            };
            let request: serde_json::Value = serde_json::from_str(&request).unwrap();
            assert!(request["params"]["prodCodeTimeoutMs"].as_u64().unwrap() <= 100);
            let result = if index == 0 {
                tokio::time::sleep(Duration::from_millis(150)).await;
                answers::error_at(1, 1, "late", "late answer for a.rs")
            } else {
                answers::no_diagnostics()
            };
            framed
                .send(WireMessage::LspPayload(
                    json!({"jsonrpc":"2.0", "id":request["id"], "result":result}).to_string(),
                ))
                .await
                .unwrap();
        }
    });
    let mut session = LspSession {
        remote: addr,
        framed: Framed::new(
            AnyStream::connect(addr).await.unwrap(),
            ProdCodeCodec::new(),
        ),
        root: ws.root(),
        opened: HashMap::new(),
        next_id: 1,
        engine: "rust".into(),
        engine_loaded: None,
        refreshed: None,
        index_gated: false,
    };
    let budget = ValidationBudget::new();
    for (index, file) in ["a.rs", "b.rs"].into_iter().enumerate() {
        let uri = session.uri_for(std::path::Path::new(file)).unwrap();
        session.opened.insert(uri.clone(), (ws.path(file), None));
        let result = budget
            .query(
                &mut session,
                std::path::Path::new(file),
                "textDocument/diagnostic",
                json!({"textDocument":{"uri":uri}}),
                Duration::from_millis(100),
            )
            .await;
        if index == 0 {
            assert!(result.unwrap_err().to_string().contains("budget exhausted"));
        } else {
            assert_eq!(result.unwrap(), answers::no_diagnostics());
        }
    }
    peer.await.unwrap();
}

#[tokio::test]
async fn validate_batch_deadline_bounds_repeated_cancellations_and_names_unchecked_files() {
    use super::batch::validate_texts_with_budget;
    use super::budget::ValidationBudget;
    use std::time::Duration;

    let ws = Workspace::new(&[("a.rs", "fn a() {}"), ("b.rs", "fn b() {}")]);
    let proposed = Arc::new(AtomicBool::new(false));
    let gateway = ScriptedGateway::start(move |method, params| {
        if method == "textDocument/didOpen"
            && params["textDocument"]["text"]
                .as_str()
                .is_some_and(|t| t.contains("proposed"))
        {
            proposed.store(true, Ordering::SeqCst);
        }
        if method == "textDocument/diagnostic" {
            return if proposed.load(Ordering::SeqCst) {
                answers::failure("cancelled because of pending write")
            } else {
                answers::no_diagnostics()
            };
        }
        json!([])
    })
    .await;
    let mut budget = ValidationBudget::new();
    budget.deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    budget.diagnostic = Duration::from_secs(10);
    let reports = tokio::time::timeout(
        Duration::from_secs(3),
        validate_texts_with_budget(
            gateway.addr(),
            &ws.root(),
            &[
                ("a.rs".into(), "fn proposed_a() {}".into()),
                ("b.rs".into(), "fn proposed_b() {}".into()),
            ],
            &[],
            &budget,
        ),
    )
    .await
    .expect("the batch deadline must include retries")
    .unwrap();
    assert_eq!(reports.len(), 2);
    for (report, file) in reports.iter().zip(["a.rs", "b.rs"]) {
        assert_eq!(report.file, file);
        assert!(!report.ok());
        assert!(
            report.items[0].message.contains("budget exhausted"),
            "{report:?}"
        );
    }
}

#[tokio::test]
async fn validate_batch_slow_baselines_leave_time_for_proposed_diagnostics() {
    use super::batch::validate_texts_with_budget;
    use super::budget::ValidationBudget;
    use std::time::Duration;

    let ws = Workspace::new(&[("a.rs", "fn a() {}"), ("b.rs", "fn b() {}")]);
    let proposed = Arc::new(AtomicBool::new(false));
    let baseline_queries = Arc::new(AtomicUsize::new(0));
    let proposal_queries = Arc::new(AtomicUsize::new(0));
    let gateway = ScriptedGateway::start({
        let baseline_queries = Arc::clone(&baseline_queries);
        let proposal_queries = Arc::clone(&proposal_queries);
        move |method, params| {
            if method == "textDocument/didOpen"
                && params["textDocument"]["text"]
                    .as_str()
                    .is_some_and(|t| t.contains("proposed"))
            {
                proposed.store(true, Ordering::SeqCst);
            }
            if method == "textDocument/diagnostic" {
                if !proposed.load(Ordering::SeqCst) {
                    baseline_queries.fetch_add(1, Ordering::SeqCst);
                    return answers::failure("cancelled because of pending write");
                }
                proposal_queries.fetch_add(1, Ordering::SeqCst);
                return answers::no_diagnostics();
            }
            json!([])
        }
    })
    .await;
    let mut budget = ValidationBudget::new();
    budget.auxiliary = Duration::from_millis(20);
    let reports = tokio::time::timeout(
        Duration::from_secs(3),
        validate_texts_with_budget(
            gateway.addr(),
            &ws.root(),
            &[
                ("a.rs".into(), "fn proposed_a() {}".into()),
                ("b.rs".into(), "fn proposed_b() {}".into()),
            ],
            &[],
            &budget,
        ),
    )
    .await
    .expect("baseline retries must have their own small budget")
    .unwrap();
    assert!(baseline_queries.load(Ordering::SeqCst) >= 2);
    assert_eq!(proposal_queries.load(Ordering::SeqCst), 2);
    assert!(reports.iter().all(|r| r.ok()), "{reports:?}");
}
