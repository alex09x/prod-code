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
use prod_code_testkit::{ScriptedGateway, Workspace};
use serde_json::json;

fn text(result: McpToolCallResult) -> String {
    assert!(!result.is_error);
    match &result.content[..] {
        [crate::protocol::McpContentItem::Text { text }] => text.clone(),
        other => panic!("unexpected content: {other:?}"),
    }
}

#[tokio::test]
async fn hover_renders_supported_contents_and_saturates_positions() {
    for (reply, expected) in [
        (json!({"contents":{"value":"**number**"}}), "**number**"),
        (
            json!({"contents":[{"value":"one"},"ignored",{"value":"two"}]}),
            "one\n\ntwo",
        ),
        (json!({"contents":[]}), ""),
        (
            json!({"contents":"plain"}),
            "No hover information available.",
        ),
        (json!(null), "No hover information available."),
    ] {
        let ws = Workspace::new(&[("sample.rs", "fn value() {}\n")]);
        let gateway = ScriptedGateway::start(move |method, params| {
            if method == "textDocument/hover" {
                assert_eq!(params["position"], json!({"line":0,"character":0}));
                reply.clone()
            } else {
                json!(null)
            }
        })
        .await;
        assert_eq!(
            text(
                handle_hover(
                    gateway.addr(),
                    &ws.root(),
                    &json!({
                        "path":"sample.rs", "line":0, "character":0
                    })
                )
                .await
                .unwrap()
            ),
            expected
        );
        assert_eq!(gateway.calls(), 1);
    }
}

#[tokio::test]
async fn hover_and_analysis_refuse_missing_arguments_in_order() {
    let ws = Workspace::new(&[]);
    let remote = "127.0.0.1:1".parse().unwrap();
    for (args, expected) in [
        (json!({}), "Missing 'path' argument"),
        (json!({"path":"a.rs"}), "Missing 'line' argument"),
        (
            json!({"path":"a.rs","line":1}),
            "Missing 'character' argument",
        ),
    ] {
        assert_eq!(
            handle_hover(remote, &ws.root(), &args)
                .await
                .unwrap_err()
                .to_string(),
            expected
        );
    }
    assert_eq!(
        handle_structural_search(&ws.root(), &json!({}))
            .await
            .unwrap_err()
            .to_string(),
        "Missing 'pattern' argument"
    );
    for (args, expected) in [
        (json!({}), "Missing 'path' argument"),
        (json!({"path":"a.rs"}), "Missing 'line' argument"),
        (
            json!({"path":"a.rs","line":1}),
            "Missing 'target_type' argument",
        ),
    ] {
        assert_eq!(
            handle_propose_expression(&ws.root(), &args)
                .await
                .unwrap_err()
                .to_string(),
            expected
        );
    }
    assert_eq!(
        handle_structural_search(&ws.root(), &json!({"pattern":""}))
            .await
            .unwrap_err()
            .to_string(),
        "pattern cannot be empty"
    );
}

#[tokio::test]
async fn sync_reports_counts_and_remote_root() {
    let ws = Workspace::new(&[("sample.rs", "fn sample() {}\n")]);
    let gw = ScriptedGateway::start(|_, _| json!(null)).await;
    assert_eq!(
        text(
            handle_sync(gw.addr(), &ws.root(), json!({"path":"sample.rs"}))
                .await
                .unwrap()
        ),
        format!(
            "⚡ Fast-Sync Completed\n• Files planned: 1\n• Files updated: 0\n• Files deleted: 0\n• Data transferred: 0.0 KB\n• Remote workspace: {}",
            ws.root().display()
        )
    );
}

async fn status_reply(reply: Option<WireMessage>) -> McpToolCallResult {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let serve = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut framed = Framed::new(stream, ProdCodeCodec::new());
        assert!(matches!(
            framed.next().await.unwrap().unwrap(),
            WireMessage::StatusRequest
        ));
        if let Some(reply) = reply {
            framed.send(reply).await.unwrap();
        }
    });
    let result = handle_status(addr).await.unwrap();
    serve.await.unwrap();
    result
}

#[tokio::test]
async fn status_renders_metrics_and_reports_wrong_or_closed_responses() {
    let result = text(
        status_reply(Some(WireMessage::StatusResponse(
            prod_code_protocol::StatusResponse {
                server_pid: 42,
                uptime_seconds: 3661,
                active_sessions: 2,
                loaded_workspaces: 3,
                total_queries: 7,
                active_queries: 1,
                detected_engines: vec!["rust".into(), "python".into()],
                memory_rss_bytes: Some(2 * 1024 * 1024),
                ..Default::default()
            },
        )))
        .await,
    );
    let (_, metrics) = result.split_once(" RTT)\n").unwrap();
    assert_eq!(
        metrics,
        "• Server PID: 42\n• Uptime: 1h 1m 1s\n• Memory RSS: 2.00 MB\n• Active Sessions: 2\n• Running Commands: 0\n• Loaded Workspaces: 3\n• Queries Handled: 7 (in-flight: 1)\n• Engines: rust, python\n• Status: HEALTHY"
    );
    let closed = status_reply(None).await;
    assert!(closed.is_error);
    let crate::protocol::McpContentItem::Text { text: closed_text } = &closed.content[0];
    assert_eq!(
        closed_text,
        "Gateway closed connection without status response"
    );
    let wrong = status_reply(Some(WireMessage::StatusRequest)).await;
    assert!(wrong.is_error);
    let crate::protocol::McpContentItem::Text { text: wrong_text } = &wrong.content[0];
    assert_eq!(
        wrong_text,
        "Unexpected response from gateway: StatusRequest"
    );
}

#[tokio::test]
async fn status_describes_reported_pressure_and_running_commands() {
    let result = text(
        status_reply(Some(WireMessage::StatusResponse(
            prod_code_protocol::StatusResponse {
                host: prod_code_protocol::HostResources {
                    memory_available_bytes: Some(5),
                    memory_total_bytes: Some(100),
                    storage_free_millis: Some(40),
                },
                running_commands: vec![prod_code_protocol::RunningCommand {
                    workspace: "sample".into(),
                    command: "cargo test".into(),
                    running_seconds: 65,
                }],
                ..Default::default()
            },
        )))
        .await,
    );
    let (_, metrics) = result.split_once(" RTT)\n").unwrap();
    assert_eq!(
        metrics,
        "• Server PID: 0\n• Uptime: 0h 0m 0s\n• Memory RSS: 0.00 MB\n• Host: memory 95% used, disk 4% free\n• Active Sessions: 0\n• Running Commands: 1\n    - sample  1m 5s  cargo test\n• Loaded Workspaces: 0\n• Queries Handled: 0 (in-flight: 0)\n• Engines: \n• Status: SHORT (memory 95% used, disk 4% free): new workspaces go to other nodes"
    );
}

#[tokio::test]
async fn local_inspection_handlers_forward_options_and_render_results() {
    let ws = Workspace::new(&[("sample.rs", "fn value() {\n    run();\n}\n")]);
    for args in [
        json!({}),
        json!({"min_lines":10,"parameterized":false,"type3":true,"max_groups":1,"path":ws.path("sample.rs")}),
    ] {
        assert_eq!(
            text(handle_find_duplicates(&ws.root(), &args).await.unwrap()),
            "⚡ prod-code Clone & Duplication Harvester Report\n────────────────────────────────────────────────────\nFiles Scanned: 1 | Lines: 3 | Clone Groups: 0 | Duplication: 0.0%\n\n✓ No duplicate code blocks detected exceeding threshold.\n"
        );
    }
    for args in [
        json!({"scope":"modules"}),
        json!({"scope":"modules","path":ws.path("sample.rs")}),
    ] {
        let report = text(handle_dependencies(&ws.root(), &args).await.unwrap());
        assert_eq!(
            report.lines().nth(2),
            Some("Scope: modules | Nodes: 1 | Dependencies: 0")
        );
        assert_eq!(
            report.lines().last(),
            Some("Isolated (Leaf/Orphan) Nodes (1): sample")
        );
    }
    let report = text(
        handle_structural_search(&ws.root(), &json!({"pattern":"run()","path":"sample.rs"}))
            .await
            .unwrap(),
    );
    assert_eq!(
        report.lines().next(),
        Some("⚡ prod-code Structural AST Search: `run()`")
    );
    assert_eq!(report.lines().nth(4), Some("  • sample.rs:2:5  run()"));
    assert_eq!(
        text(
            handle_propose_expression(
                &ws.root(),
                &json!({"path":"sample.rs","line":1,"target_type":"Unknown"})
            )
            .await
            .unwrap()
        ),
        "⚡ prod-code Type-Directed Expression Synthesis\n────────────────────────────────────────────────────\nTarget Type: `Unknown` at sample.rs:1\nCandidates Found: 0\n\nNo in-scope expressions found producing target type.\n"
    );
    assert_eq!(
        text(with_indexing_notes(Ok(McpToolCallResult::text("answer")), &ws.root()).unwrap()),
        "answer"
    );
    assert_eq!(
        with_indexing_notes(Err(anyhow::anyhow!("failed query")), &ws.root())
            .unwrap_err()
            .to_string(),
        "failed query"
    );
    let empty = Workspace::new(&[]);
    for args in [json!({}), json!({"scope":"unknown"})] {
        let report = text(handle_dependencies(&empty.root(), &args).await.unwrap());
        assert_eq!(
            report.lines().nth(2),
            Some("Scope: modules | Nodes: 0 | Dependencies: 0")
        );
    }
}
