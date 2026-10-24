/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::sync::atomic::Ordering;
use std::time::Duration;

use super::fake_server::fake_engine;
use crate::types::lock_unpoisoned;

#[tokio::test]
async fn completed_response_survives_server_exit() {
    let (dir, engine) = fake_engine(None, Duration::from_secs(10)).await;
    let seen = dir.path().join("seen");
    let id = engine.next_req_id.load(Ordering::Relaxed);
    let mut request = Box::pin(engine.send_request("prodCode/silence", serde_json::json!({})));
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            tokio::select! {
                result = &mut request => panic!("request completed before its reply: {result:?}"),
                _ = tokio::time::sleep(Duration::from_millis(5)) => {
                    if std::fs::read_to_string(&seen).unwrap_or_default()
                        .lines().any(|line| line == "prodCode/silence") { break; }
                }
            }
        }
    })
    .await
    .expect("the owned fake server receives the request");

    // The production reader can deliver the response and observe EOF before this waiter runs.
    let expected = serde_json::json!({"result": {"contents": "completed"}});
    let sender = lock_unpoisoned(&engine.pending_requests)
        .remove(&id)
        .unwrap();
    sender.send(expected.clone()).unwrap();
    engine.retire_generation();
    let actual = request.await;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if lock_unpoisoned(&engine._child)
                .try_wait()
                .unwrap()
                .is_some()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the owned server is reaped");
    assert_eq!(
        actual.expect("a received response survives server retirement"),
        expected
    );
    assert!(
        engine
            .send_request("textDocument/hover", serde_json::json!({}))
            .await
            .is_err(),
        "new requests must still reject a retired engine"
    );
}
