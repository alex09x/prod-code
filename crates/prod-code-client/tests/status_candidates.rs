/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{ProdCodeCodec, StatusResponse, WireMessage};
use std::process::Output;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio_util::codec::Framed;

async fn status(root: &std::path::Path, remotes: &str) -> Output {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_prod-code"));
    command
        .current_dir(root)
        .args(["--remote", remotes, "status", "--json"])
        .env_remove("HOME")
        .kill_on_drop(true);
    tokio::time::timeout(Duration::from_secs(8), command.output())
        .await
        .expect("status candidate lookup must remain bounded")
        .unwrap()
}

#[tokio::test]
async fn issue_1807_status_uses_a_live_candidate_without_requiring_the_checkout_engine() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("Cargo.toml"),
        "[package]\nname = \"status-fixture\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let live = listener.local_addr().unwrap();
    let placements = Arc::new(AtomicUsize::new(0));
    let counted = placements.clone();
    let server = tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let mut framed = Framed::new(socket, ProdCodeCodec::new());
            match framed.next().await {
                Some(Ok(WireMessage::StatusRequest)) => {
                    let _ = framed
                        .send(WireMessage::StatusResponse(StatusResponse {
                            detected_engines: vec!["swift".into()],
                            platform: Some("macos aarch64".into()),
                            ..Default::default()
                        }))
                        .await;
                }
                Some(Ok(WireMessage::PlaceRequest(_))) => {
                    counted.fetch_add(1, Ordering::SeqCst);
                }
                _ => {}
            }
        }
    });
    let remotes = format!("[::1]:{},{}", live.port(), live);
    let output = status(root.path(), &remotes).await;
    server.abort();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["remote"], live.to_string());
    assert_eq!(placements.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn issue_1807_status_names_all_unreachable_explicit_candidates() {
    let root = tempfile::tempdir().unwrap();
    let output = status(root.path(), "[::1]:0,127.0.0.1:0").await;
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("[::1]:0"), "{stderr}");
    assert!(stderr.contains("127.0.0.1:0"), "{stderr}");
}
