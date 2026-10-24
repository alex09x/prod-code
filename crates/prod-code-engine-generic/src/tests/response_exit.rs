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

use crate::config::GenericLspConfig;
use crate::engine::GenericLspEngine;
use crate::types::lock_unpoisoned;

#[tokio::test]
async fn completed_response_survives_server_exit() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("server.py");
    let seen = dir.path().join("request-seen");
    std::fs::write(
        &script,
        r#"import json, sys
while True:
    length = 0
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            sys.exit(0)
        if not line.strip():
            break
        if line.lower().startswith(b"content-length:"):
            length = int(line.split(b":")[1])
    request = json.loads(sys.stdin.buffer.read(length))
    if request.get("method") == "initialize":
        response = json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": {"capabilities": {}}}).encode()
        sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(response) + response)
        sys.stdout.buffer.flush()
    elif request.get("method") == "prodCode/silence":
        with open(sys.argv[1], "w") as marker:
            marker.write("received")
"#,
    )
    .unwrap();
    let engine = GenericLspEngine::spawn(
        dir.path(),
        GenericLspConfig {
            command: "python3".to_string(),
            args: vec![
                script.to_string_lossy().into_owned(),
                seen.to_string_lossy().into_owned(),
            ],
            request_timeout: Duration::from_secs(10),
            health_probe_interval: None,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let id = engine.next_req_id.load(Ordering::Relaxed);
    let mut request = Box::pin(engine.send_request("prodCode/silence", serde_json::json!({})));
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            tokio::select! {
                result = &mut request => panic!("request completed before its reply: {result:?}"),
                _ = tokio::time::sleep(Duration::from_millis(5)) => {
                    if seen.exists() { break; }
                }
            }
        }
    })
    .await
    .expect("the owned fake server receives the request");

    // Deliver a full response, then retire the server before polling the waiter again.
    let expected = serde_json::json!({"result": {"contents": "completed"}});
    let sender = lock_unpoisoned(&engine.pending_requests)
        .remove(&id)
        .unwrap();
    sender.send(expected.clone()).unwrap();
    engine.is_alive.store(false, Ordering::Release);
    lock_unpoisoned(&engine._child).start_kill().unwrap();
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
