/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Manifest-probe deletions must invalidate or forget the workspace search index before
//! `SyncProbeResponse` is sent, so an immediate `SearchRequest` never returns symbols from
//! files deleted by `apply_sync_probe`.

use futures_util::{SinkExt, StreamExt};
use prod_code_gateway::{ServerState, handle_client};
use prod_code_protocol::{
    FileDelta, FileStamp, ProdCodeCodec, SearchRequest, SearchResponse, SyncProbeRequest,
    SyncProbeResponse, SyncRequest, SyncResponse, WireMessage, content_hash,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio_util::codec::Framed;

struct GatewayHarness {
    _dir: tempfile::TempDir,
    state: Arc<ServerState>,
    client: Framed<TcpStream, ProdCodeCodec>,
    server: tokio::task::JoinHandle<anyhow::Result<()>>,
}

impl GatewayHarness {
    async fn start() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let storage = dir.path().join("storage");
        std::fs::create_dir_all(&storage).expect("create storage dir");
        let state = Arc::new(ServerState::new(storage));
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        let client_stream = TcpStream::connect(addr).await.expect("connect");
        let server_state = Arc::clone(&state);
        let server = tokio::spawn(async move {
            let (socket, peer) = listener.accept().await.expect("accept");
            handle_client(socket, peer, server_state).await
        });
        Self {
            _dir: dir,
            state,
            client: Framed::new(client_stream, ProdCodeCodec::new()),
            server,
        }
    }

    async fn ask(&mut self, msg: WireMessage) -> WireMessage {
        self.client.send(msg).await.expect("send request");
        loop {
            let frame = tokio::time::timeout(Duration::from_secs(20), self.client.next())
                .await
                .expect("response timeout")
                .expect("connection closed")
                .expect("frame decode");
            if !matches!(frame, WireMessage::Pong) {
                return frame;
            }
        }
    }

    async fn sync(
        &mut self,
        client_root: &str,
        base_name: Option<&str>,
        files: Vec<FileDelta>,
    ) -> SyncResponse {
        let answer = self
            .ask(WireMessage::SyncRequest(SyncRequest {
                client_workspace_root: client_root.to_string(),
                files,
                clean_others: false,
                base_workspace_name: base_name.map(str::to_string),
            }))
            .await;
        let WireMessage::SyncResponse(resp) = answer else {
            panic!("expected SyncResponse, got {answer:?}");
        };
        resp
    }

    async fn probe(
        &mut self,
        client_root: &str,
        base_name: Option<&str>,
        files: Vec<FileStamp>,
    ) -> SyncProbeResponse {
        let answer = self
            .ask(WireMessage::SyncProbeRequest(SyncProbeRequest {
                client_workspace_root: client_root.to_string(),
                base_workspace_name: base_name.map(str::to_string),
                seed_from: None,
                files,
            }))
            .await;
        let WireMessage::SyncProbeResponse(resp) = answer else {
            panic!("expected SyncProbeResponse, got {answer:?}");
        };
        resp
    }

    async fn search(
        &mut self,
        client_root: &str,
        base_name: Option<&str>,
        query: &str,
    ) -> SearchResponse {
        let answer = self
            .ask(WireMessage::SearchRequest(SearchRequest {
                client_workspace_root: client_root.to_string(),
                base_workspace_name: base_name.map(str::to_string),
                query: query.to_string(),
                limit: 10,
                subpath: None,
                client_agent: None,
                client_host: None,
            }))
            .await;
        let WireMessage::SearchResponse(resp) = answer else {
            panic!("expected SearchResponse, got {answer:?}");
        };
        resp
    }

    async fn shutdown(mut self) {
        self.client
            .send(WireMessage::Disconnect {
                reason: "test complete".to_string(),
            })
            .await
            .expect("send disconnect");
        assert!(self.client.next().await.is_none());
        self.server
            .await
            .expect("server join")
            .expect("handle_client ok");
    }
}

fn delta(path: &str, content: &[u8]) -> FileDelta {
    FileDelta {
        relative_path: path.to_string(),
        content: Some(content.to_vec()),
        is_executable: false,
    }
}

fn stamp(path: &str, content: &[u8]) -> FileStamp {
    FileStamp {
        relative_path: path.to_string(),
        size: content.len() as u64,
        hash: content_hash(content),
    }
}

#[tokio::test]
async fn manifest_probe_deletion_removes_stale_search_declarations_before_acknowledging() {
    let mut gw = GatewayHarness::start().await;
    let client_root = "/client/probe-search-ws";
    let base_name = Some("probe-search-ws");

    let keep_src = b"/// Stays present in kept file.\npub fn surviving_anchor_entry() {}\n";
    let deleted_src =
        b"/// Removed when omitted from manifest.\npub fn obsolete_manifest_target() {}\n";
    let other_ws_src = b"/// Unrelated workspace item.\npub fn isolated_neighbor_item() {}\n";

    // Populate target workspace and an unrelated workspace (without base_workspace_name) to
    // verify path identity isolation.
    let sync_resp = gw
        .sync(
            client_root,
            base_name,
            vec![
                delta("src/keep.rs", keep_src),
                delta("src/deleted.rs", deleted_src),
            ],
        )
        .await;
    assert_eq!(sync_resp.files_updated, 2);

    let other_sync = gw
        .sync(
            "/client/other-ws",
            None,
            vec![delta("src/other.rs", other_ws_src)],
        )
        .await;
    assert_eq!(other_sync.files_updated, 1);

    // 1. Build the search index before manifest reconciliation.
    let before_deleted = gw
        .search(client_root, base_name, "obsolete_manifest_target")
        .await;
    assert!(before_deleted.error.is_none(), "{before_deleted:?}");
    assert_eq!(before_deleted.indexed_files, 2);
    assert_eq!(before_deleted.indexed_declarations, 2);
    assert_eq!(before_deleted.hits.len(), 1);
    assert_eq!(before_deleted.hits[0].name, "obsolete_manifest_target");
    assert_eq!(before_deleted.hits[0].file, "src/deleted.rs");

    let before_other = gw
        .search("/client/other-ws", None, "isolated_neighbor_item")
        .await;
    assert_eq!(before_other.indexed_files, 1);
    assert_eq!(before_other.hits.len(), 1);

    // 2. Reconcile a manifest that omits `src/deleted.rs`.
    let probe_resp = gw
        .probe(client_root, base_name, vec![stamp("src/keep.rs", keep_src)])
        .await;
    assert!(!probe_resp.seeded);
    assert_eq!(probe_resp.files_deleted, 1);
    assert!(probe_resp.missing.is_empty());

    // 3. Immediately search: deleted symbols must disappear while unaffected files remain.
    let after_deleted = gw
        .search(client_root, base_name, "obsolete_manifest_target")
        .await;
    assert!(after_deleted.error.is_none(), "{after_deleted:?}");
    assert!(
        after_deleted.hits.is_empty(),
        "stale symbol survived manifest probe deletion: {:?}",
        after_deleted.hits
    );
    assert_eq!(after_deleted.indexed_files, 1);
    assert_eq!(after_deleted.indexed_declarations, 1);

    let after_kept = gw
        .search(client_root, base_name, "surviving_anchor_entry")
        .await;
    assert!(after_kept.error.is_none(), "{after_kept:?}");
    assert_eq!(after_kept.indexed_files, 1);
    assert_eq!(after_kept.indexed_declarations, 1);
    assert_eq!(after_kept.hits.len(), 1);
    assert_eq!(after_kept.hits[0].name, "surviving_anchor_entry");
    assert_eq!(after_kept.hits[0].file, "src/keep.rs");

    // 4. Zero-deletion probe preserves the built index without forcing a full tree walk.
    let server_ws = prod_code_gateway::workspace::server_workspace_path(
        &gw.state.storage_root,
        client_root,
        base_name,
    );
    let unannounced_src = b"/// Written outside sync to verify zero-deletion probe keeps built index.\npub fn unannounced_disk_only() {}\n";
    std::fs::write(server_ws.join("src/unannounced.rs"), unannounced_src)
        .expect("write unannounced");
    let zero_probe = gw
        .probe(
            client_root,
            base_name,
            vec![
                stamp("src/keep.rs", keep_src),
                stamp("src/unannounced.rs", unannounced_src),
            ],
        )
        .await;
    assert_eq!(zero_probe.files_deleted, 0);
    assert!(zero_probe.missing.is_empty());
    assert_eq!(
        zero_probe.server_workspace_root,
        server_ws.to_string_lossy()
    );

    let after_zero_probe = gw
        .search(client_root, base_name, "surviving_anchor_entry")
        .await;
    assert_eq!(
        after_zero_probe.indexed_files, 1,
        "zero-deletion probe must keep the existing built index rather than walking disk"
    );
    assert_eq!(after_zero_probe.hits.len(), 1);
    assert_eq!(after_zero_probe.hits[0].name, "surviving_anchor_entry");

    // Clean up `src/unannounced.rs` via a probe that omits it.
    let cleanup_probe = gw
        .probe(client_root, base_name, vec![stamp("src/keep.rs", keep_src)])
        .await;
    assert_eq!(cleanup_probe.files_deleted, 1);

    // 5. Verify path identity when base_workspace_name is None.
    let second_root_src =
        b"/// Ephemeral symbol in root-addressed workspace.\npub fn transient_checkout_leaf() {}\n";
    gw.sync(
        "/client/other-ws",
        None,
        vec![delta("src/ephemeral.rs", second_root_src)],
    )
    .await;
    let other_before = gw
        .search("/client/other-ws", None, "transient_checkout_leaf")
        .await;
    assert_eq!(other_before.indexed_files, 2);
    assert_eq!(other_before.hits.len(), 1);

    let other_probe = gw
        .probe(
            "/client/other-ws",
            None,
            vec![stamp("src/other.rs", other_ws_src)],
        )
        .await;
    assert_eq!(other_probe.files_deleted, 1);

    let other_after_deleted = gw
        .search("/client/other-ws", None, "transient_checkout_leaf")
        .await;
    assert!(
        other_after_deleted.hits.is_empty(),
        "stale symbol survived in root-addressed workspace: {:?}",
        other_after_deleted.hits
    );
    assert_eq!(other_after_deleted.indexed_files, 1);

    let other_after_kept = gw
        .search("/client/other-ws", None, "isolated_neighbor_item")
        .await;
    assert_eq!(other_after_kept.hits.len(), 1);
    assert_eq!(other_after_kept.hits[0].name, "isolated_neighbor_item");

    gw.shutdown().await;
}
