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
use crate::editor_proxy::probe::CHANNEL_CAPACITY;
use crate::workspace::WatchedChange;

async fn saturate(session: &mut Session) -> Instant {
    let started = Instant::now();
    // Larger than an OS pipe: the writer holds frame zero while the child waits.
    session
        .send(json!({"jsonrpc":"2.0", "id":0, "method":"test/frame",
        "params":{"padding":"x".repeat(1024 * 1024)}}))
        .await;
    for id in 1..=CHANNEL_CAPACITY {
        session
            .send(json!({"jsonrpc":"2.0", "id":id, "method":"test/frame"}))
            .await;
    }
    session
        .wait_until(|| {
            let registrations = session.servers.servers.lock().unwrap();
            registrations
                .first()
                .is_some_and(|s| s.input.len() == CHANNEL_CAPACITY)
        })
        .await;
    session
        .send(json!({"jsonrpc":"2.0", "id":CHANNEL_CAPACITY + 1,
        "method":"test/frame"}))
        .await;
    // Liveness is recorded at the actual editor dispatch boundary, before enqueue.
    session
        .wait_until(|| {
            session.state.lock().unwrap().valid_evidence_epoch >= (CHANNEL_CAPACITY + 2) as u64
        })
        .await;
    assert_eq!(
        session.servers.count(),
        1,
        "full input queue must preserve the session while capacity is awaited"
    );
    started
}

#[tokio::test]
async fn editor_proxy_issue_1289_temporary_saturation_preserves_order_and_session() {
    let mut session = Session::start(None, WAIT).await;
    saturate(&mut session).await;
    session.release();
    for id in 0..=CHANNEL_CAPACITY + 1 {
        let response = session.receive().await;
        assert_eq!(response["id"], id, "ordered frame {id}");
        assert_eq!(response["result"]["method"], "test/frame");
    }
    session
        .send(json!({"jsonrpc":"2.0", "id":9000, "method":"test/after-drain"}))
        .await;
    assert_eq!(session.receive().await["id"], 9000);
    session.editor.send(WireMessage::Ping).await.unwrap();
    assert!(matches!(
        timeout(WAIT, session.editor.next()).await.unwrap(),
        Some(Ok(WireMessage::Pong))
    ));
    session.disconnect().await;
}

#[tokio::test]
async fn editor_proxy_issue_1289_permanent_stall_has_one_write_budget() {
    let budget = Duration::from_millis(700);
    let mut session = Session::start(None, budget).await;
    let started = saturate(&mut session).await;
    session.finish().await;
    let elapsed = started.elapsed();
    assert!(
        elapsed >= budget,
        "temporary fullness must not disconnect early: {elapsed:?}"
    );
    assert!(
        elapsed < budget + TEARDOWN + Duration::from_secs(1),
        "deadline was not reset: {elapsed:?}"
    );
}

#[tokio::test]
async fn editor_proxy_issue_1289_registration_retirement_interrupts_capacity_wait() {
    let mut session = Session::start(None, WAIT).await;
    saturate(&mut session).await;
    let retired = Instant::now();
    session
        .servers
        .notify(&[(
            session.root.path().join("changed.ts"),
            WatchedChange::Changed,
        )])
        .await;
    session.finish().await;
    assert!(
        retired.elapsed() < Duration::from_secs(1),
        "retirement waited for the write budget"
    );
}

#[tokio::test]
async fn editor_proxy_issue_1289_child_exit_interrupts_capacity_wait() {
    let mut session = Session::start(None, WAIT).await;
    saturate(&mut session).await;
    let exited = Instant::now();
    std::fs::write(session.root.path().join("exit"), "exit").unwrap();
    session.finish().await;
    assert!(
        exited.elapsed() < Duration::from_secs(1),
        "child exit waited for the write budget"
    );
}
