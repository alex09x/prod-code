/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::Result;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crate::workspace::manager::WorkspaceManager;
use crate::workspace::shared::SharedWorkspace;
use crate::workspace::types::{RustLoader, unix_now};

pub(crate) async fn generic_workspace(root: &Path) -> (Arc<SharedWorkspace>, PathBuf) {
    let script = root.join("language-server.py");
    std::fs::write(
        &script,
        r#"import json, sys
def read():
    length = 0
    while True:
        line = sys.stdin.buffer.readline()
        if not line: return None
        line = line.strip()
        if not line: break
        if line.lower().startswith(b"content-length:"): length = int(line.split(b":")[1])
    return json.loads(sys.stdin.buffer.read(length)) if length else None
def send(value):
    body = json.dumps(value).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
    sys.stdout.buffer.flush()
while True:
    message = read()
    if message is None: break
    if message.get("method") == "initialize":
        send({"jsonrpc":"2.0", "id":message["id"], "result":{"capabilities":{}}})
"#,
    )
    .unwrap();
    let config = prod_code_engine_generic::GenericLspConfig {
        command: "python3".to_string(),
        args: vec![script.to_string_lossy().into_owned()],
        ..Default::default()
    };
    let engine = prod_code_engine_generic::GenericLspEngine::spawn(root, config)
        .await
        .expect("fake generic engine starts");
    (
        Arc::new(SharedWorkspace::new(
            root.to_path_buf(),
            "python".to_string(),
            None,
            None,
            Some(Arc::new(engine)),
            None,
        )),
        script,
    )
}

/// A manager admitting against the host memory `snapshots` report in turn, counting every
/// new engine at 2 GiB.
pub(crate) fn manager_on(snapshots: Vec<(u64, u64)>) -> Arc<WorkspaceManager> {
    Arc::new(WorkspaceManager::with_admission(Arc::new(
        crate::admission::Admission::with_probe(
            crate::admission::scripted_probe(snapshots),
            2048,
            crate::admission::LOAD_SETTLE,
        ),
    )))
}

/// `used` of `total` GiB in use.
pub(crate) fn host(used: u64, total: u64) -> (u64, u64) {
    const GIB: u64 = 1 << 30;
    ((total - used) * GIB, total * GIB)
}

/// A loaded workspace at `root`, without a session for `idle_secs`.
pub(crate) fn loaded(root: &str, idle_secs: u64) -> Arc<SharedWorkspace> {
    let ws = Arc::new(SharedWorkspace::new(
        PathBuf::from(root),
        "text".to_string(),
        None,
        None,
        None,
        None,
    ));
    ws.last_used
        .store(unix_now() - idle_secs, Ordering::Relaxed);
    ws
}

/// A Rust loader that reports each load as it starts, holds it until the test lets one
/// through (or drops the gate), and then fails, so that no real engine is built.
pub(crate) struct SlowLoader {
    pub(crate) load: RustLoader,
    pub(crate) started: tokio::sync::mpsc::UnboundedReceiver<()>,
    pub(crate) gate: std::sync::mpsc::Sender<()>,
    pub(crate) loads: Arc<AtomicUsize>,
}

pub(crate) fn slow_loader() -> SlowLoader {
    let (started_tx, started) = tokio::sync::mpsc::unbounded_channel();
    let (gate, gate_rx) = std::sync::mpsc::channel::<()>();
    let gate_rx = std::sync::Mutex::new(gate_rx);
    let loads = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&loads);
    SlowLoader {
        load: Arc::new(
            move |_root: &Path| -> Result<prod_code_engine_rust::RustEngine> {
                counted.fetch_add(1, Ordering::SeqCst);
                let _ = started_tx.send(());
                let _ = gate_rx.lock().unwrap().recv();
                anyhow::bail!("scripted load")
            },
        ),
        started,
        gate,
        loads,
    }
}

/// Admission on a host with 10 of 100 GiB in use, 2 GiB a new engine, returned as soon as
/// its load ends.
pub(crate) fn roomy_admission() -> Arc<crate::admission::Admission> {
    Arc::new(crate::admission::Admission::with_probe(
        crate::admission::scripted_probe(vec![host(10, 100)]),
        2048,
        Duration::ZERO,
    ))
}
