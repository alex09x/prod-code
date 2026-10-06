/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::oneshot;

use crate::config::GenericLspConfig;
use crate::engine::GenericLspEngine;
use crate::types::{PendingRequest, lock_unpoisoned};

#[tokio::test]
async fn exit_details_waits_for_final_stderr_drain() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = dir.path().join("exiting-server.py");
    std::fs::write(
        &script,
        r#"import json, sys
length = 0
while True:
    line = sys.stdin.buffer.readline()
    if not line:
        sys.exit(2)
    if not line.strip():
        break
    if line.lower().startswith(b"content-length:"):
        length = int(line.split(b":")[1])
request = json.loads(sys.stdin.buffer.read(length))
response = json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": {"capabilities": {}}}).encode()
sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(response) + response)
sys.stdout.buffer.flush()
sys.stderr.write("final diagnostic from startup\n")
sys.stderr.flush()
sys.exit(7)
"#,
    )
    .expect("fake server");

    let engine = GenericLspEngine::spawn(
        dir.path(),
        GenericLspConfig {
            command: "python3".to_string(),
            args: vec![script.to_string_lossy().into_owned()],
            ..Default::default()
        },
    )
    .await
    .expect("initialize before the fake server exits");

    tokio::time::timeout(Duration::from_secs(2), async {
        while engine.is_alive() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the fake server exits");
    let details = engine.exit_details().await.expect("exit details");
    assert!(details.contains("exit code 7"), "{details}");
    assert!(
        details.contains("final diagnostic from startup"),
        "{details}"
    );
}

#[tokio::test]
async fn exit_details_surfaces_partial_stderr_when_drain_times_out() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = dir.path().join("hanging-stderr-server.py");
    std::fs::write(
        &script,
        r#"import json, subprocess, sys
length = 0
while True:
    line = sys.stdin.buffer.readline()
    if not line:
        sys.exit(2)
    if not line.strip():
        break
    if line.lower().startswith(b"content-length:"):
        length = int(line.split(b":")[1])
request = json.loads(sys.stdin.buffer.read(length))
response = json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": {"capabilities": {}}}).encode()
sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(response) + response)
sys.stdout.buffer.flush()

subprocess.Popen([sys.executable, "-c", "import time; time.sleep(10)"], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=sys.stderr)
sys.stderr.write("partial diagnostic before timeout\n")
sys.stderr.flush()
sys.exit(7)
"#,
    )
    .expect("fake server");

    let engine = GenericLspEngine::spawn(
        dir.path(),
        GenericLspConfig {
            command: "python3".to_string(),
            args: vec![script.to_string_lossy().into_owned()],
            ..Default::default()
        },
    )
    .await
    .expect("initialize before the fake server exits");

    tokio::time::timeout(Duration::from_secs(4), async {
        while engine.is_alive() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the fake server exits");
    let details = engine.exit_details().await.expect("exit details");
    assert!(details.contains("exit code 7"), "{details}");
    assert!(
        details.contains("partial diagnostic before timeout"),
        "{details}"
    );
    assert!(details.contains("stderr [incomplete drain]"), "{details}");
}

#[tokio::test]
async fn dropping_request_ownership_cleans_pending_and_retires_only_partial_frames() {
    let spawn_child = || {
        let mut command = Command::new("python3");
        command.kill_on_drop(true);
        command
            .args(["-c", "import time; time.sleep(60)"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("supervised child")
    };

    for (frame_written, retired) in [(true, false), (false, true)] {
        let pending: Arc<StdMutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>> =
            Arc::new(StdMutex::new(HashMap::new()));
        let (tx, _rx) = oneshot::channel();
        lock_unpoisoned(&pending).insert(7, tx);
        let child = Arc::new(StdMutex::new(spawn_child()));
        let is_alive = Arc::new(AtomicBool::new(true));
        drop(PendingRequest {
            id: 7,
            pending: Arc::clone(&pending),
            child: Arc::clone(&child),
            is_alive: Arc::clone(&is_alive),
            frame_written,
        });

        assert!(lock_unpoisoned(&pending).is_empty());
        assert_eq!(!is_alive.load(Ordering::Relaxed), retired);
        let _ = lock_unpoisoned(&child).start_kill();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queued_request_rechecks_retirement_before_writing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = dir.path().join("queued-server.py");
    let seen = dir.path().join("seen");
    std::fs::write(
        &script,
        r#"import json, os, sys
def read():
    length = 0
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.strip()
        if not line:
            break
        if line.lower().startswith(b"content-length:"):
            length = int(line.split(b":")[1])
    return json.loads(sys.stdin.buffer.read(length))
def send(message):
    body = json.dumps(message).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
    sys.stdout.buffer.flush()
while True:
    message = read()
    if message is None:
        break
    method = message.get("method", "")
    with open(os.environ["FAKE_SEEN_FILE"], "a") as log:
        log.write(method + "\n")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": message["id"], "result": {"capabilities": {}}})
    elif "id" in message:
        send({"jsonrpc": "2.0", "id": message["id"], "result": "unexpected success"})
"#,
    )
    .expect("fake server");
    let mut config = GenericLspConfig {
        command: "python3".to_string(),
        args: vec![script.to_string_lossy().into_owned()],
        ..Default::default()
    };
    config.env.insert(
        "FAKE_SEEN_FILE".to_string(),
        seen.to_string_lossy().into_owned(),
    );
    let engine = Arc::new(
        GenericLspEngine::spawn(dir.path(), config)
            .await
            .expect("the fake server starts"),
    );
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
    let methods = std::fs::read_to_string(seen).expect("request log");
    assert!(
        !methods
            .lines()
            .any(|method| method == "prodCode/queuedAfterRetirement"),
        "{methods}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queued_notifications_and_requests_preserve_healthy_state() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = dir.path().join("notification-server.py");
    std::fs::write(
        &script,
        r#"import json, sys
def read():
    length = 0
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.strip()
        if not line:
            break
        if line.lower().startswith(b"content-length:"):
            length = int(line.split(b":")[1])
    return json.loads(sys.stdin.buffer.read(length))
def send(message):
    body = json.dumps(message).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
    sys.stdout.buffer.flush()
seen = []
while True:
    message = read()
    if message is None:
        break
    seen.append(message.get("method"))
    if message.get("method") == "initialize":
        send({"jsonrpc": "2.0", "id": message["id"], "result": {"capabilities": {}}})
    elif "id" in message:
        send({"jsonrpc": "2.0", "id": message["id"], "result": seen})
"#,
    )
    .expect("fake server");
    let config = GenericLspConfig {
        command: "python3".to_string(),
        args: vec![script.to_string_lossy().into_owned()],
        request_timeout: Duration::from_secs(2),
        ..Default::default()
    };
    let mut engine = GenericLspEngine::spawn(dir.path(), config).await.unwrap();
    engine.config.request_timeout = Duration::from_millis(50);
    let engine = Arc::new(engine);
    let documents = engine.documents.lock().await;
    let uri = "file:///queued.py";
    let queued = {
        let engine = Arc::clone(&engine);
        tokio::spawn(async move {
            engine
                .send_notification(
                    "textDocument/didOpen",
                    serde_json::json!({"textDocument":{"uri":uri,"languageId":"python","version":1,"text":"queued\n"}}),
                )
                .await
        })
    };
    let error = queued
        .await
        .expect("notification task")
        .expect_err("the document lock consumes the notification budget");
    assert!(format!("{error:#}").contains("textDocument/didOpen"));
    assert!(!documents.documents.contains_key(uri));
    assert!(!engine.sent.read().await.contains_key(uri));
    assert!(engine.accepts_documents());
    assert!(engine.is_alive());
    drop(documents);

    engine
        .send_notification(
            "textDocument/didOpen",
            serde_json::json!({"textDocument":{"uri":uri,"languageId":"python","version":1,"text":"sent\n"}}),
        )
        .await
        .expect("a later notification remains healthy");

    // Hold the writer explicitly: serialization speed must not determine test order.
    let writer = engine.stdin.lock().await;
    let error = engine
        .send_request("prodCode/queued", serde_json::json!({}))
        .await
        .expect_err("writer contention must consume the request budget");
    assert!(format!("{error:#}").contains("Timeout"));
    assert!(lock_unpoisoned(&engine.pending_requests).is_empty());
    assert!(engine.is_alive());
    drop(writer);
    let seen = engine
        .send_request("prodCode/seen", serde_json::json!({}))
        .await
        .unwrap();
    assert!(
        !seen["result"]
            .as_array()
            .unwrap()
            .iter()
            .any(|method| method == "prodCode/queued")
    );
}
