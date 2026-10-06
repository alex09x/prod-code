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
use std::sync::atomic::Ordering;
use std::time::Duration;

use crate::types::lock_unpoisoned;

#[cfg(unix)]
use super::fake_server::{assert_process_exits, fake_engine};

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn request_deadline_covers_a_blocked_full_frame_write() {
    let (_dir, engine) =
        fake_engine(Some(("FAKE_STOP_READING", "1")), Duration::from_millis(200)).await;
    let observed = tokio::time::timeout(
        Duration::from_secs(2),
        engine.send_request(
            "prodCode/large",
            serde_json::json!({ "payload": "x".repeat(8 * 1024 * 1024) }),
        ),
    )
    .await;
    let error = observed
        .expect("the internal deadline includes a blocked write")
        .expect_err("the full frame cannot be written");
    let text = format!("{error:#}");
    assert!(text.contains("prodCode/large"), "{text}");
    assert!(text.to_lowercase().contains("timeout"), "{text}");
    assert!(!engine.is_alive(), "the partial stream is retired");
    assert!(lock_unpoisoned(&engine.pending_requests).is_empty());
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn writer_lock_timeout_never_sends_the_expired_request() {
    // Coverage instrumentation makes a fresh Python handshake slower than ordinary test
    // code, while the occupied writer still exercises the same request deadline path.
    let (dir, engine) = fake_engine(None, Duration::from_secs(1)).await;
    let writer = engine.stdin.lock().await;
    let waiting = {
        let engine = Arc::clone(&engine);
        tokio::spawn(async move {
            engine
                .send_request("prodCode/queued", serde_json::json!({}))
                .await
        })
    };
    let error = waiting
        .await
        .expect("request task")
        .expect_err("the request expires behind the writer");
    let text = format!("{error:#}");
    assert!(text.contains("prodCode/queued"), "{text}");
    assert!(text.to_lowercase().contains("timeout"), "{text}");
    drop(writer);

    let seen = engine
        .send_request("prodCode/seen", serde_json::json!({}))
        .await
        .expect("healthy response after contention");
    assert!(
        !seen["result"]
            .as_array()
            .expect("methods")
            .iter()
            .any(|method| method == "prodCode/queued"),
        "{seen}"
    );
    assert!(lock_unpoisoned(&engine.pending_requests).is_empty());
    let methods = std::fs::read_to_string(dir.path().join("seen")).expect("request log");
    assert!(!methods.lines().any(|method| method == "prodCode/queued"));
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queued_request_rechecks_retirement_before_writing() {
    let (dir, engine) = fake_engine(None, Duration::from_secs(2)).await;
    let writer = engine.stdin.lock().await;
    let before = engine.next_req_id.load(Ordering::Relaxed);
    let waiting = {
        let engine = Arc::clone(&engine);
        tokio::spawn(async move {
            engine
                .send_request("prodCode/queuedAfterRetirement", serde_json::json!({}))
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(2), async {
        while engine.next_req_id.load(Ordering::Relaxed) == before {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the request reached the occupied writer");

    engine.is_alive.store(false, Ordering::Release);
    drop(writer);
    let error = waiting
        .await
        .expect("request task")
        .expect_err("a retired engine cannot answer successfully");
    assert!(
        format!("{error:#}").contains("exited before request"),
        "{error:#}"
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    let seen = std::fs::read_to_string(dir.path().join("seen")).expect("request log");
    assert!(
        !seen
            .lines()
            .any(|method| method == "prodCode/queuedAfterRetirement"),
        "{seen}"
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn write_error_keeps_context_and_retires_the_child() {
    let (_dir, engine) = fake_engine(Some(("FAKE_CLOSE_STDIN", "1")), Duration::from_secs(2)).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let error = engine
        .send_request(
            "prodCode/brokenWrite",
            serde_json::json!({ "payload": "x".repeat(1024 * 1024) }),
        )
        .await
        .expect_err("stdin was closed");
    let text = format!("{error:#}");
    assert!(text.contains("prodCode/brokenWrite"), "{text}");
    assert!(text.to_lowercase().contains("write"), "{text}");
    assert!(!engine.is_alive());
    assert!(lock_unpoisoned(&engine.pending_requests).is_empty());
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_a_partial_frame_retires_the_child_and_wakes_waiters() {
    let (_dir, engine) =
        fake_engine(Some(("FAKE_STOP_READING", "1")), Duration::from_secs(10)).await;
    let writing = {
        let engine = Arc::clone(&engine);
        tokio::spawn(async move {
            engine
                .send_request(
                    "prodCode/cancelled",
                    serde_json::json!({ "payload": "x".repeat(8 * 1024 * 1024) }),
                )
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    let waiting = {
        let engine = Arc::clone(&engine);
        tokio::spawn(async move {
            engine
                .send_request("prodCode/waiting", serde_json::json!({}))
                .await
        })
    };
    writing.abort();
    assert!(writing.await.expect_err("cancelled").is_cancelled());
    let error = tokio::time::timeout(Duration::from_secs(2), waiting)
        .await
        .expect("the waiter is woken")
        .expect("waiter task")
        .expect_err("the child was retired");
    assert!(
        format!("{error:#}").contains("prodCode/waiting"),
        "{error:#}"
    );
    assert!(!engine.is_alive());
    assert!(lock_unpoisoned(&engine.pending_requests).is_empty());
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn complete_frame_cancellation_cleans_pending_and_keeps_concurrency_healthy() {
    let (_dir, engine) = fake_engine(None, Duration::from_secs(2)).await;
    let cancelled = {
        let engine = Arc::clone(&engine);
        tokio::spawn(async move {
            engine
                .send_request("prodCode/delay", serde_json::json!({}))
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    cancelled.abort();
    assert!(cancelled.await.expect_err("cancelled").is_cancelled());
    assert!(lock_unpoisoned(&engine.pending_requests).is_empty());

    let first = {
        let engine = Arc::clone(&engine);
        tokio::spawn(async move {
            engine
                .send_request("textDocument/hover", serde_json::json!({}))
                .await
        })
    };
    let second = {
        let engine = Arc::clone(&engine);
        tokio::spawn(async move {
            engine
                .send_request("textDocument/hover", serde_json::json!({}))
                .await
        })
    };
    for response in [first, second] {
        assert_eq!(
            response.await.expect("task").expect("response")["result"]["contents"],
            "healthy"
        );
    }
    assert!(engine.is_alive());
    assert!(lock_unpoisoned(&engine.pending_requests).is_empty());
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notification_deadline_covers_a_blocked_frame() {
    let (dir, engine) =
        fake_engine(Some(("FAKE_STOP_READING", "1")), Duration::from_millis(200)).await;
    let outcome = tokio::time::timeout(
        Duration::from_secs(2),
        engine.send_notification("textDocument/didOpen", serde_json::json!({
            "textDocument": {"uri": "file:///notification.go", "languageId":"go", "version":1, "text": "x".repeat(8 * 1024 * 1024)}
        })),
    ).await.expect("notification must honor its internal write budget");
    let error = outcome.expect_err("the server never reads the notification");
    let text = format!("{error:#}");
    assert!(text.contains("textDocument/didOpen"), "{text}");
    assert!(text.to_lowercase().contains("timeout"), "{text}");
    assert!(
        !engine.is_alive(),
        "a partial document frame cannot be reused"
    );
    assert_process_exits(&dir.path().join("pid")).await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_a_notification_retires_its_owned_process() {
    let (dir, engine) =
        fake_engine(Some(("FAKE_STOP_READING", "1")), Duration::from_secs(10)).await;
    let writing = {
        let engine = Arc::clone(&engine);
        tokio::spawn(async move {
            engine
                .send_notification(
                    "prodCode/cancelledNotification",
                    serde_json::json!({"payload": "x".repeat(8 * 1024 * 1024)}),
                )
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    writing.abort();
    assert!(writing.await.expect_err("cancelled").is_cancelled());
    assert!(!engine.is_alive());
    assert_process_exits(&dir.path().join("pid")).await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_automatic_response_retires_gopls() {
    let (dir, engine) = fake_engine(
        Some(("FAKE_HUGE_AUTO_REQUEST", "1")),
        Duration::from_millis(200),
    )
    .await;
    tokio::time::timeout(Duration::from_secs(2), async {
        while engine.is_alive() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the automatic response has a bounded write");
    assert_process_exits(&dir.path().join("pid")).await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn complete_notification_keeps_gopls_healthy() {
    let (_dir, engine) = fake_engine(None, Duration::from_secs(2)).await;
    engine
        .send_notification("workspace/didChangeConfiguration", serde_json::json!({}))
        .await
        .unwrap();
    let response = engine
        .send_request("textDocument/hover", serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(response["result"]["contents"], "healthy");
    assert!(engine.is_alive());
}

#[cfg(unix)]
#[tokio::test]
async fn optional_response_headers_keep_gopls_messages_aligned() {
    for order in ["before", "after"] {
        let (_dir, engine) =
            fake_engine(Some(("FAKE_CONTENT_TYPE", order)), Duration::from_secs(2)).await;
        let response = engine
            .send_request("textDocument/hover", serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(response["result"]["contents"], "healthy", "{order}");
        assert!(engine.is_alive());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn configuration_replies_match_items_and_refuse_invalid_params() {
    let (_dir, engine) = fake_engine(None, Duration::from_secs(2)).await;
    for items in [
        serde_json::json!([]),
        serde_json::json!([{}]),
        serde_json::json!([
            {"section": "gopls"},
            {"section": "unknown", "scopeUri": "file:///workspace/main.go"},
            {"scopeUri": "file:///workspace/second.go"}
        ]),
    ] {
        let response = engine
            .send_request(
                "prodCode/configurationReply",
                serde_json::json!({"configuration": {"items": items}}),
            )
            .await
            .unwrap();
        let reply = &response["result"];
        assert_eq!(reply["id"], "server-config-é");
        assert!(reply.get("error").is_none(), "{reply}");
        assert_eq!(
            reply["result"],
            serde_json::json!(vec![serde_json::json!({}); items.as_array().unwrap().len()]),
            "one default setting is required per requested item"
        );
    }
    for configuration in [
        serde_json::json!(null),
        serde_json::json!({}),
        serde_json::json!({"items": null}),
        serde_json::json!({"items": {}}),
        serde_json::json!({"items": [null]}),
        serde_json::json!({"items": [{"section": 7}]}),
        serde_json::json!({"items": [{"scopeUri": false}]}),
    ] {
        let response = engine
            .send_request(
                "prodCode/configurationReply",
                serde_json::json!({"configuration": configuration}),
            )
            .await
            .unwrap();
        let reply = &response["result"];
        assert_eq!(reply["id"], "server-config-é");
        assert_eq!(reply["error"]["code"], -32602, "{reply}");
        assert!(
            reply["error"]["message"]
                .as_str()
                .unwrap()
                .contains("workspace/configuration")
        );
        assert!(reply.get("result").is_none(), "{reply}");
    }
    let hover = engine
        .send_request("textDocument/hover", serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(hover["result"]["contents"], "healthy");
    assert!(engine.is_alive());
}
