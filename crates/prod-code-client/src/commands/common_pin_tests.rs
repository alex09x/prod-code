/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use futures_util::StreamExt;
use prod_code_protocol::{ProdCodeCodec, WireMessage};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio_util::codec::Framed;

pub(crate) struct OldPinnedCheckout {
    root: tempfile::TempDir,
    pins: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for OldPinnedCheckout {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl OldPinnedCheckout {
    pub(crate) async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::write(
            root.path().join("Cargo.toml"),
            "[package]\nname = \"pin-target\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        std::fs::write(root.path().join("src/lib.rs"), "pub fn visible() {}\n").unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        std::fs::write(
            root.path().join("prod-code.toml"),
            format!("[cluster]\npin = \"{addr}\"\n"),
        )
        .unwrap();
        let pins = Arc::new(AtomicUsize::new(0));
        let seen = pins.clone();
        let task = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let mut framed = Framed::new(socket, ProdCodeCodec::new());
                if matches!(
                    framed.next().await,
                    Some(Ok(WireMessage::NodePinRequest(_)))
                ) {
                    seen.fetch_add(1, Ordering::SeqCst);
                }
                // A legacy gateway cannot decode the pin: it closes before any work.
            }
        });
        Self { root, pins, task }
    }

    pub(crate) fn root(&self) -> &Path {
        self.root.path()
    }
    pub(crate) fn file(&self) -> PathBuf {
        self.root.path().join("src/lib.rs")
    }
    pub(crate) fn pin_requests(&self) -> usize {
        self.pins.load(Ordering::SeqCst)
    }
    pub(crate) fn other_node(&self) -> SocketAddr {
        "127.0.0.1:1".parse().unwrap()
    }
}

#[tokio::test]
async fn issue_1793_cli_outer_failover_uses_the_target_checkouts_pin() {
    let target = OldPinnedCheckout::new().await;
    let rediscovered = AtomicUsize::new(0);
    let result = super::run_tool_in_with(
        target.other_node(),
        target.root(),
        "code_outline",
        serde_json::json!({ "path": "src/lib.rs" }),
        |_, _| {
            rediscovered.fetch_add(1, Ordering::SeqCst);
            async { None }
        },
    )
    .await;
    assert!(result.is_err());
    assert_eq!(target.pin_requests(), 1);
    assert_eq!(
        rediscovered.load(Ordering::SeqCst),
        0,
        "a failed target pin must not run unpinned discovery from the caller checkout"
    );
}
