/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{
    ExecChunk, ExecExit, ExecRequest, HostResources, ProdCodeCodec, StatusResponse,
    SyncProbeResponse, SyncResponse, WireMessage,
};
use tokio_util::codec::Framed;

async fn gateway(exit: i32) -> (SocketAddr, tokio::task::JoinHandle<ExecRequest>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            let mut wire = Framed::new(socket, ProdCodeCodec::new());
            while let Some(Ok(message)) = wire.next().await {
                match message {
                    WireMessage::StatusRequest => {
                        wire.send(WireMessage::StatusResponse(StatusResponse {
                            server_pid: std::process::id(),
                            uptime_seconds: 1,
                            active_sessions: 0,
                            loaded_workspaces: 0,
                            detected_engines: vec!["rust".into()],
                            memory_rss_bytes: None,
                            total_queries: 0,
                            active_queries: 0,
                            load_average_millis: Some(0),
                            cpu_count: Some(4),
                            platform: None,
                            running_commands: vec![],
                            host: HostResources::default(),
                            version: None,
                            git_commit: None,
                        }))
                        .await
                        .unwrap();
                        break;
                    }
                    WireMessage::SyncProbeRequest(request) => wire
                        .send(WireMessage::SyncProbeResponse(SyncProbeResponse {
                            server_workspace_root: request.client_workspace_root,
                            seeded: false,
                            files_deleted: 0,
                            missing: vec![],
                        }))
                        .await
                        .unwrap(),
                    WireMessage::SyncRequest(request) => wire
                        .send(WireMessage::SyncResponse(SyncResponse {
                            server_workspace_root: request.client_workspace_root,
                            files_updated: 0,
                            files_deleted: 0,
                            bytes_transferred: 0,
                            duration_ms: 0,
                            workspace_was_fresh: false,
                            stale_paths: vec![],
                        }))
                        .await
                        .unwrap(),
                    WireMessage::ExecRequest(request) => {
                        let state = std::fs::read_to_string(
                            std::path::Path::new(&request.client_workspace_root).join("src/lib.rs"),
                        )
                        .unwrap();
                        assert!(
                            state.contains("{ 2 }"),
                            "verification must see the applied change"
                        );
                        let output = if exit == 0 {
                            "test workflow_regression ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n"
                        } else {
                            "test workflow_regression ... FAILED\ntest result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n"
                        };
                        wire.send(WireMessage::ExecChunk(ExecChunk {
                            stderr: false,
                            data: Some(output.as_bytes().to_vec()),
                        }))
                        .await
                        .unwrap();
                        wire.send(WireMessage::ExecExit(ExecExit {
                            exit_code: Some(exit),
                            duration_ms: 1,
                            server_workspace_root: request.client_workspace_root.clone(),
                            timed_out: false,
                            error: None,
                            usage: None,
                            platform: None,
                        }))
                        .await
                        .unwrap();
                        return request;
                    }
                    unexpected => panic!("unexpected verification message: {unexpected:?}"),
                }
            }
        }
    });
    (address, task)
}

#[tokio::test]
async fn full_chain_sends_the_scoped_test_request_and_preserves_failed_verification() {
    for exit in [0, 1] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname=\"verification_fixture\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
        )
        .unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn answer() -> u32 { 1 }\n").unwrap();
        let (remote, task) = gateway(exit).await;
        let result = tokio::time::timeout(std::time::Duration::from_secs(3), execute(remote, root, json!({
            "apply":true,"timeout_ms":2000,"steps":[
                {"id":"proposal","tool":"preview","args":{"edits":[{"path":"src/lib.rs","old":"{ 1 }","new":"{ 2 }"}]}},
                {"id":"write","tool":"apply","args":{"files":{"$ref":"proposal#/files"}}},
                {"id":"check","tool":"verify","args":{"kind":"test","path":"src/lib.rs","filter":"workflow_regression","timeout_secs":1}}
            ]
        }))).await.unwrap().unwrap();
        let report = super::test_support::report(&result);
        assert_eq!(
            report["steps"][1]["effects"]["status"], "applied",
            "{report}"
        );
        assert_eq!(
            report["steps"][2]["effects"]["status"],
            if exit == 0 { "passed" } else { "failed" },
            "{report}"
        );
        assert_eq!(
            report["status"],
            if exit == 0 { "complete" } else { "partial" },
            "{report}"
        );
        let request = tokio::time::timeout(std::time::Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap();
        assert!(
            request
                .command
                .iter()
                .any(|argument| argument == "workflow_regression")
        );
        assert!(
            !request
                .command
                .iter()
                .any(|argument| argument == "--workspace")
        );
        assert_eq!(
            std::fs::read_to_string(root.join("src/lib.rs")).unwrap(),
            "pub fn answer() -> u32 { 2 }\n"
        );
        crate::sync::clear_sync_cache(root);
    }
}
