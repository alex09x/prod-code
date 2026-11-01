/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::test_support::socket_pair;
use super::*;
use serde_json::json;
use std::{collections::VecDeque, sync::Arc};

#[tokio::test]
async fn replay_refuses_incremental_sync_to_full_sync_downgrade() {
    let (socket, mut peer) = socket_pair().await;
    let (mut tx, mut rx) = socket.split();
    let temp = tempfile::tempdir().unwrap();
    let files = prod_code_client::editor_files::RemoteFiles::new(
        "127.0.0.1:1".parse().unwrap(),
        temp.path(),
        temp.path(),
        temp.path(),
    );
    let tracker = LspStateTracker {
        initialize_req: Some(json!({"id":1,"method":"initialize"}).to_string()),
        ..Default::default()
    };
    let (_sender, receiver) = tokio::sync::mpsc::channel(1);
    let task = tokio::spawn(async move {
        let _ = peer.next().await;
        peer.send(WireMessage::LspPayload(
            json!({"id":1,"result":{"capabilities":{"textDocumentSync":{"change":1}}}}).to_string(),
        ))
        .await
        .unwrap();
    });
    // UTF-16 in the low bits; 3 in the high field denotes incremental sync.
    let encoding = AtomicU8::new(3 << 2);
    let result = replay_lsp_state(
        &mut tx,
        &mut rx,
        &tracker,
        true,
        &files,
        &encoding,
        &Arc::new(tokio::sync::Mutex::new(receiver)),
        &mut VecDeque::new(),
        &Arc::new(tokio::sync::Mutex::new(Vec::new())),
        &tokio::sync::Mutex::new(tokio::io::stdout()),
        &None,
    )
    .await;
    assert!(result.unwrap_err().to_string().contains("sync"));
    task.await.unwrap();
}
