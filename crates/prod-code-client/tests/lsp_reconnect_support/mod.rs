/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_client::editor_files::read_frame;
use serde_json::Value;
use std::net::SocketAddr;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::BufReader;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

pub fn spawn_bridge(
    ws: &prod_code_testkit::Workspace,
    cache: &Path,
    addr: SocketAddr,
    trace: Option<&Path>,
) -> tokio::process::Child {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_prod-code"));
    command
        .args(["--remote", &addr.to_string(), "lsp", "--watchdog-secs", "0"])
        .env("HOME", cache)
        .env("XDG_CACHE_HOME", cache.join(".cache"))
        .env_remove("PROD_CODE_CLUSTER")
        .env_remove("PROD_CODE_RECONNECT")
        .current_dir(ws.root())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if let Some(path) = trace {
        command.env("PROD_CODE_LSP_TRACE", path);
    } else {
        command.env_remove("PROD_CODE_LSP_TRACE");
    }
    command.spawn().unwrap()
}

pub async fn interruptible_proxy(
    upstream: SocketAddr,
) -> (
    SocketAddr,
    watch::Sender<u8>,
    Arc<std::sync::atomic::AtomicUsize>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (down, state) = watch::channel(0_u8);
    let blackholes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed_blackholes = Arc::clone(&blackholes);
    tokio::spawn(async move {
        loop {
            let (mut inbound, _) = listener.accept().await.unwrap();
            let mut state = state.clone();
            let mode = *state.borrow();
            if mode == 1 {
                continue;
            }
            if mode == 2 {
                observed_blackholes.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    drop(inbound);
                });
                continue;
            }
            tokio::spawn(async move {
                let Ok(mut outbound) = TcpStream::connect(upstream).await else {
                    return;
                };
                tokio::select! {
                    _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound) => {}
                    _ = async {
                        while state.changed().await.is_ok() {
                            if *state.borrow() != 0 { break; }
                        }
                    } => {}
                }
            });
        }
    });
    (addr, down, blackholes)
}

pub async fn response(stdout: &mut BufReader<tokio::process::ChildStdout>, id: u64) -> Value {
    tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            let frame = read_frame(stdout)
                .await
                .unwrap()
                .expect("LSP process exited");
            let message: Value = serde_json::from_str(&frame).unwrap();
            if message["id"] == id {
                return message;
            }
        }
    })
    .await
    .expect("timed out waiting for LSP response")
}
