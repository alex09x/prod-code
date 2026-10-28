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
use crate::state::SessionMeta;
use crate::workspace::session::SessionView;
use crate::workspace::shared::SharedWorkspace;
use prod_code_protocol::PathTranslator;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[tokio::test]
async fn an_unloaded_workspace_disconnects_its_existing_session_before_the_next_request() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("workspace");
    std::fs::create_dir_all(&root).unwrap();
    let workspace = Arc::new(SharedWorkspace::new(
        root.clone(),
        "rust".to_string(),
        None,
        None,
        None,
        None,
    ));
    workspace.unloaded.store(true, Ordering::SeqCst);
    let view = SessionView {
        session_id: 1,
        worktree_root: root.clone(),
        workspace: Arc::clone(&workspace),
        accounted: workspace,
        is_single_owner: Arc::new(std::sync::atomic::AtomicBool::new(true)),
        direct_edit_open_files: Arc::new(Mutex::new(HashMap::new())),
        editor_caps: Default::default(),
        lease: None,
        owner: None,
    };
    let root_text = root.to_string_lossy().into_owned();
    let meta = Arc::new(SessionMeta {
        session_id: 1,
        client_name: "test".into(),
        agent: "test".into(),
        host: "test".into(),
        client_addr: "test".into(),
        workspace: root_text.clone(),
        engine: "rust".into(),
        engine_root: root.clone(),
        storage_root: temp.path().to_path_buf(),
        metrics: Arc::new(crate::metrics::Metrics::new(temp.path().join("metrics"))),
        editor: false,
        redirects: false,
        edits: Arc::new(Mutex::new(HashMap::new())),
    });
    let (raw_out, mut output_rx) = rapidfire::mpsc::bounded(4);
    let out = SharedOutputSender::new(raw_out, Duration::from_secs(1));
    let translator = PathTranslator::new(&root_text, &root_text);
    let pending = Arc::new(tokio::sync::Mutex::new(HashMap::new()));

    let flow = on_client_message(
        Some(Ok(WireMessage::LspPayload("{}".into()))),
        &out,
        &translator,
        &view,
        &meta,
        &pending,
    )
    .await;

    assert!(matches!(flow, Flow::Stop));
    assert!(matches!(
        output_rx.recv().await.unwrap().message,
        WireMessage::Disconnect { ref reason }
            if reason == "workspace project model changed; reconnect"
    ));
}
