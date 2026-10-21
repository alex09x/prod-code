/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A sync and the probe after it answer while the workspace's Rust engine stays locked by a
//! stuck query, and the engine takes the synced file once it is free (#1143).

use prod_code_gateway::workspace::{SharedWorkspace, WorkspaceManager};
use prod_code_gateway::{apply_sync, apply_sync_probe};
use prod_code_protocol::{FileDelta, FileStamp, SyncProbeRequest, SyncRequest, content_hash};
use std::sync::Arc;
use tokio::time::{Duration, Instant, timeout};

/// Well above the gateway's bounded wait for a busy engine, well below the client's
/// 180-second sync timeout.
const ANSWER_LIMIT: Duration = Duration::from_secs(20);

const SYNCED: &str = "pub const INITIAL_VAL: u32 = 1;\npub fn added_after_busy_sync() {}\n";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sync_and_probe_answer_while_a_query_holds_the_rust_engine() {
    let storage = tempfile::tempdir().unwrap();
    let client = tempfile::tempdir().unwrap();
    let client_root = std::fs::canonicalize(client.path()).unwrap();
    let client_root = client_root.to_string_lossy().to_string();
    let server_root =
        prod_code_gateway::workspace::server_workspace_path(storage.path(), &client_root, None);
    std::fs::create_dir_all(server_root.join("src")).unwrap();
    std::fs::write(
        server_root.join("Cargo.toml"),
        "[package]\nname = \"busy_engine_sync\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\npath = \"src/lib.rs\"\n",
    )
    .unwrap();
    let lib = server_root.join("src/lib.rs");
    std::fs::write(&lib, "pub const INITIAL_VAL: u32 = 1;\n").unwrap();

    let engine = prod_code_engine_rust::RustEngine::load(&server_root).expect("load engine");
    let engine = Arc::new(tokio::sync::Mutex::new(engine));
    let workspace = Arc::new(SharedWorkspace::new(
        server_root.clone(),
        "rust".to_string(),
        Some(Arc::clone(&engine)),
        None,
        None,
        None,
    ));
    let manager = WorkspaceManager::with_admission(Arc::new(
        prod_code_gateway::admission::Admission::unbounded(),
    ));
    manager.insert_ready_for_test(Arc::clone(&workspace)).await;

    // A query that never returns keeps the engine locked, as on the node in #1143.
    let stuck_query = Arc::clone(&engine).lock_owned().await;

    let started = Instant::now();
    let response = timeout(
        ANSWER_LIMIT,
        apply_sync(
            storage.path(),
            &manager,
            SyncRequest {
                client_workspace_root: client_root.clone(),
                files: vec![FileDelta {
                    relative_path: "src/lib.rs".to_string(),
                    content: Some(SYNCED.as_bytes().to_vec()),
                    is_executable: false,
                }],
                clean_others: false,
                base_workspace_name: None,
            },
        ),
    )
    .await
    .expect("a sync must be answered while the engine is busy");
    assert_eq!(response.files_updated, 1, "{response:?}");
    assert!(response.stale_paths.is_empty(), "{response:?}");
    assert_eq!(std::fs::read_to_string(&lib).unwrap(), SYNCED);
    eprintln!(
        "sync answered in {:?} with the engine locked",
        started.elapsed()
    );

    // The next pre-flight starts with a probe; it takes the same workspace sync lock.
    let probe = timeout(
        ANSWER_LIMIT,
        apply_sync_probe(
            storage.path(),
            &manager,
            SyncProbeRequest {
                client_workspace_root: client_root.clone(),
                base_workspace_name: None,
                seed_from: None,
                files: ["Cargo.toml", "src/lib.rs"]
                    .into_iter()
                    .map(|rel| {
                        let bytes = std::fs::read(server_root.join(rel)).unwrap();
                        FileStamp {
                            relative_path: rel.to_string(),
                            size: bytes.len() as u64,
                            hash: content_hash(&bytes),
                        }
                    })
                    .collect(),
            },
        ),
    )
    .await
    .expect("the probe after a sync must be answered while the engine is busy");
    assert!(probe.missing.is_empty(), "{probe:?}");

    // Once the query lets go, the engine sees the synced text without another sync.
    drop(stuck_query);
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let names: Vec<String> = engine
            .lock()
            .await
            .document_symbols(&lib)
            .unwrap_or_default()
            .into_iter()
            .map(|symbol| symbol.name)
            .collect();
        if names.iter().any(|name| name == "added_after_busy_sync") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the deferred base update never reached the engine: {names:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
