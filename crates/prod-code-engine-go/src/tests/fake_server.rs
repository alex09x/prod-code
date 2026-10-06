/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tempfile::tempdir;

use crate::config::{
    DEFAULT_HEALTH_PROBE_INTERVAL, GoConfig, HEALTH_PROBE_METHOD, find_gopls_binary,
};
use crate::engine::GoEngine;

#[cfg(unix)]
pub(crate) const FAKE_GOPLS: &str = r#"#!/usr/bin/env python3
import json, os, sys, threading, time

LOCK = threading.Lock()
SEEN = []
INITIALIZE_COUNT = 0
SEEN_FILE = os.environ.get("FAKE_SEEN_FILE")
if os.environ.get("FAKE_PID_FILE"):
    with open(os.environ["FAKE_PID_FILE"], "w") as pid_file:
        pid_file.write(str(os.getpid()))

def send(message):
    body = json.dumps(message).encode()
    with LOCK:
        header = b"Content-Length: %d\r\n" % len(body)
        content_type = b"Content-Type: application/vscode-jsonrpc; charset=utf-8\r\n"
        if os.environ.get("FAKE_CONTENT_TYPE") == "before":
            header = content_type + header
        elif os.environ.get("FAKE_CONTENT_TYPE") == "after":
            header += content_type
        sys.stdout.buffer.write(header + b"\r\n")
        sys.stdout.buffer.write(body)
        sys.stdout.buffer.flush()

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
    body = b""
    while len(body) < length:
        chunk = sys.stdin.buffer.read(min(length - len(body), 4096))
        if not chunk:
            return None
        body += chunk
        if os.environ.get("FAKE_SLOW_READ"):
            time.sleep(float(os.environ["FAKE_SLOW_READ"]))
    return json.loads(body)

while True:
    message = read()
    if message is None:
        break
    method = message.get("method", "")
    SEEN.append(method)
    if SEEN_FILE:
        with open(SEEN_FILE, "a") as seen:
            seen.write(method + "\n")
    if method == "initialize":
        INITIALIZE_COUNT += 1
        configured = os.environ.get("FAKE_INITIALIZE_RESPONSE")
        if INITIALIZE_COUNT > 1:
            configured = os.environ.get("FAKE_INITIALIZE_AFTER_FIRST", configured)
            if os.environ.get("FAKE_INITIALIZE_DELAY_AFTER_FIRST"):
                time.sleep(float(os.environ["FAKE_INITIALIZE_DELAY_AFTER_FIRST"]))
        if configured == "delayed":
            time.sleep(0.2)
            configured = None
        if configured == "silence":
            continue
        if configured:
            response = json.loads(configured)
            response.update({"jsonrpc": "2.0", "id": message["id"]})
            send(response)
        else:
            capabilities = {} if os.environ.get("FAKE_EMPTY_CAPABILITIES") else {"hoverProvider": True}
            send({"jsonrpc": "2.0", "id": message["id"], "result": {"capabilities": capabilities}})
    elif method == "initialized" and os.environ.get("FAKE_CLOSE_STDIN"):
        os.close(0)
        threading.Event().wait()
    elif method == "initialized" and os.environ.get("FAKE_HUGE_AUTO_REQUEST"):
        send({"jsonrpc": "2.0", "id": 7001, "method": "x" * (8 * 1024 * 1024), "params": {}})
        threading.Event().wait()
    elif method == "initialized" and os.environ.get("FAKE_STOP_READING"):
        threading.Event().wait()
    elif method == "prodCode/configurationReply":
        configuration_query = message["id"]
        send({"jsonrpc": "2.0", "id": "server-config-é", "method": "workspace/configuration", "params": message["params"]["configuration"]})
    elif message.get("id") == "server-config-é" and not method:
        send({"jsonrpc": "2.0", "id": configuration_query, "result": message})
    elif method == "prodCode/brokenFrame":
        kind = message["params"]["kind"]
        data = {
            "duplicate": b"Content-Length: 2\r\nContent-Length: 2\r\n\r\n{}",
            "oversize": b"Content-Length: 268435457\r\n\r\n",
            "header": b"A" * 65537,
            "truncated": b"Content-Length: 10\r\n\r\n{}",
        }[kind]
        sys.stdout.buffer.write(data)
        sys.stdout.buffer.flush()
        if kind == "truncated":
            sys.exit(0)
        threading.Event().wait()
    elif method == "textDocument/hover":
        send({"jsonrpc": "2.0", "id": message["id"], "result": {"contents": "healthy"}})
    elif method == "prodCode/delay":
        threading.Timer(0.4, send, ({"jsonrpc": "2.0", "id": message["id"], "result": "late"},)).start()
    elif method == "prodCode/silence":
        pass
    elif method == "prodCode/healthProbe":
        health = os.environ.get("FAKE_HEALTH", "error")
        if health == "silence":
            pass
        elif health == "success":
            send({"jsonrpc": "2.0", "id": message["id"], "result": None})
        elif health == "malformed":
            send({"jsonrpc": "2.0", "id": message["id"], "unexpected": True})
        elif health == "delay":
            threading.Timer(float(os.environ.get("FAKE_HEALTH_DELAY", "0.2")), send, ({"jsonrpc": "2.0", "id": message["id"], "result": None},)).start()
        elif health == "delay-malformed":
            threading.Timer(float(os.environ.get("FAKE_HEALTH_DELAY", "0.2")), send, ({"jsonrpc": "2.0", "id": message["id"], "unexpected": True},)).start()
        else:
            send({"jsonrpc": "2.0", "id": message["id"], "error": {"code": -32601, "message": "unknown method"}})
    elif method == "prodCode/seen":
        send({"jsonrpc": "2.0", "id": message["id"], "result": SEEN})
    elif method == "prodCode/queuedAfterRetirement":
        send({"jsonrpc": "2.0", "id": message["id"], "result": "unexpected success"})
"#;

#[cfg(unix)]
pub(crate) async fn fake_engine(
    mode: Option<(&str, &str)>,
    timeout: Duration,
) -> (tempfile::TempDir, Arc<GoEngine>) {
    fake_engine_with_probe(mode, timeout, DEFAULT_HEALTH_PROBE_INTERVAL).await
}

#[cfg(unix)]
pub(crate) async fn fake_engine_with_probe(
    mode: Option<(&str, &str)>,
    timeout: Duration,
    probe_interval: Duration,
) -> (tempfile::TempDir, Arc<GoEngine>) {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempdir().expect("tempdir");
    let script = dir.path().join("fake-gopls");
    std::fs::write(&script, FAKE_GOPLS).expect("write fake gopls");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
        .expect("make fake gopls executable");
    let mut config = GoConfig {
        gopls_path: Some(script),
        shared_cache_dir: Some(dir.path().join("cache")),
        health_probe_interval: Some(probe_interval),
        ..Default::default()
    };
    config.extra_env.insert(
        "FAKE_SEEN_FILE".to_string(),
        dir.path().join("seen").to_string_lossy().into_owned(),
    );
    config.extra_env.insert(
        "FAKE_PID_FILE".to_string(),
        dir.path().join("pid").to_string_lossy().into_owned(),
    );
    if let Some((name, value)) = mode {
        config.extra_env.insert(name.to_string(), value.to_string());
    }
    let engine = GoEngine::load_with_request_timeout(dir.path(), config, timeout)
        .await
        .expect("the fake gopls starts");
    (dir, Arc::new(engine))
}

#[cfg(unix)]
pub(crate) async fn wait_for_gopls_probe_count(path: &Path, count: usize) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let seen = std::fs::read_to_string(path).unwrap_or_default();
            if seen
                .lines()
                .filter(|method| *method == HEALTH_PROBE_METHOD)
                .count()
                >= count
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the scheduled probes reach fake gopls");
}

#[cfg(unix)]
pub(crate) async fn assert_process_exits(pid_file: &Path) {
    let pid = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Ok(pid) = std::fs::read_to_string(pid_file) {
                break pid;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the fake server records its pid");
    tokio::time::timeout(Duration::from_secs(2), async {
        while std::process::Command::new("kill")
            .args(["-0", pid.trim()])
            .status()
            .is_ok_and(|status| status.success())
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the owned fake server exits");
}

pub(crate) fn gopls_is_available_for(test: &str) -> bool {
    let available = find_gopls_binary(None).is_some();
    if std::env::var_os("CI").is_some() || std::env::var_os("PROD_CODE_REQUIRE_ENGINES").is_some() {
        assert!(
            available,
            "gopls must be installed where native engine tests are required"
        );
    }
    if !available {
        eprintln!("SKIPPED {test}: gopls is not installed");
    }
    available
}
