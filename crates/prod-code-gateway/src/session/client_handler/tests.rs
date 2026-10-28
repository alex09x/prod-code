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
use prod_code_protocol::{
    AuthToken, ExecRequest, MetricsRequest, PlaceRequest, PullFileRequest, ReadFileRequest,
    RemoteExecCommand, RemoteExecLanguage, RemoteExecRequest, SearchRequest, SyncProbeRequest,
    SyncRequest,
};

struct Served {
    _dir: tempfile::TempDir,
    client: Framed<tokio::net::TcpStream, ProdCodeCodec>,
    server: tokio::task::JoinHandle<Result<()>>,
}

async fn serve(token: Option<&str>) -> Served {
    let dir = tempfile::tempdir().unwrap();
    let storage = dir.path().join("storage");
    std::fs::create_dir_all(&storage).unwrap();
    let mut state = ServerState::new(storage);
    state.auth_token = token.map(str::to_string);
    let state = Arc::new(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = tokio::net::TcpStream::connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    let server = tokio::spawn(async move {
        let (socket, peer) = listener.accept().await.unwrap();
        handle_client(socket, peer, state).await
    });
    Served {
        _dir: dir,
        client: Framed::new(client, ProdCodeCodec::new()),
        server,
    }
}

impl Served {
    async fn ask(&mut self, msg: WireMessage) -> WireMessage {
        self.client.send(msg).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(20), self.client.next())
            .await
            .expect("an answer")
            .expect("the connection stays open")
            .expect("a frame")
    }

    async fn ends(mut self) {
        assert!(self.client.next().await.is_none(), "the server hangs up");
        self.server.await.unwrap().unwrap();
    }
}

#[tokio::test]
async fn artifact_directory_listing_is_served_before_handshake() {
    let mut served = serve(None).await;
    let client_root = "/checkout/project";
    let copy = crate::workspace::server_workspace_path(
        &served._dir.path().join("storage"),
        client_root,
        Some("listing-fixture"),
    );
    std::fs::create_dir_all(copy.join("evidence/run")).unwrap();
    std::fs::write(copy.join("evidence/run/result.txt"), "result").unwrap();
    let answer = served
        .ask(WireMessage::PullFileRequest(PullFileRequest {
            client_workspace_root: client_root.into(),
            base_workspace_name: Some("listing-fixture".into()),
            path: "evidence/run".into(),
            offset: 0,
            list_directory: true,
        }))
        .await;
    let WireMessage::PullFileResponse(response) = answer else {
        panic!("unexpected response")
    };
    let listing: prod_code_protocol::ArtifactListing =
        serde_json::from_slice(&response.file.content.unwrap()).unwrap();
    assert!(
        listing
            .entries
            .iter()
            .any(|entry| entry.name == "result.txt" && entry.kind == "file")
    );
    assert!(!listing.truncated);
    assert_eq!(
        std::fs::read_to_string(copy.join("evidence/run/result.txt")).unwrap(),
        "result"
    );
    served
        .client
        .send(WireMessage::Disconnect {
            reason: "done".into(),
        })
        .await
        .unwrap();
    served.server.await.unwrap().unwrap();
}

/// Sends a plain HTTP request line and returns the status code and JSON body of the reply.
async fn http_get(s: Served, path: &str) -> (u16, serde_json::Value) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let Served {
        _dir,
        client,
        server,
    } = s;
    let mut raw = client.into_inner();
    raw.write_all(format!("GET {path} HTTP/1.1\r\nHost: unit\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut reply = Vec::new();
    raw.read_to_end(&mut reply).await.unwrap();
    server.await.unwrap().unwrap();
    let reply = String::from_utf8(reply).unwrap();
    let (head, body) = reply.split_once("\r\n\r\n").expect("a header block");
    assert!(head.to_lowercase().contains("application/json"), "{head}");
    let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
    (status, serde_json::from_str(body).unwrap())
}

#[tokio::test]
async fn http_probes_get_a_health_answer_or_a_protocol_mismatch_and_the_connection_ends() {
    for token in [None, Some("secret")] {
        for (path, status, key, value) in [
            ("/health", 200, "status", "ok"),
            ("/healthz", 200, "service", "prod-code-gateway"),
            ("/", 426, "error", "protocol_mismatch"),
            ("/health/deep", 426, "error", "protocol_mismatch"),
        ] {
            let (got, body) = http_get(serve(token).await, path).await;
            assert_eq!(got, status, "{token:?} {path}");
            assert_eq!(body[key], value, "{token:?} {path}");
        }
    }
}

#[tokio::test]
async fn a_token_protected_gateway_answers_only_a_connection_that_opens_with_the_token() {
    let refused = |reason: WireMessage| {
        assert!(
            matches!(&reason, WireMessage::Disconnect { reason } if reason == AUTH_REFUSED),
            "{reason:?}"
        );
    };
    let mut wrong = serve(Some("secret")).await;
    refused(
        wrong
            .ask(WireMessage::Auth(AuthToken("guess".to_string())))
            .await,
    );
    wrong.ends().await;

    let mut none = serve(Some("secret")).await;
    refused(none.ask(WireMessage::Ping).await);
    none.ends().await;

    let mut right = serve(Some("secret")).await;
    right
        .client
        .send(WireMessage::Auth(AuthToken("secret".to_string())))
        .await
        .unwrap();
    assert_eq!(right.ask(WireMessage::Ping).await, WireMessage::Pong);
    // A second token changes nothing, and neither does an unexpected message.
    right
        .client
        .send(WireMessage::Auth(AuthToken("other".to_string())))
        .await
        .unwrap();
    right.client.send(WireMessage::Pong).await.unwrap();
    assert_eq!(right.ask(WireMessage::Ping).await, WireMessage::Pong);
    right
        .client
        .send(WireMessage::Disconnect {
            reason: "done".to_string(),
        })
        .await
        .unwrap();
    right.ends().await;
}

#[tokio::test]
async fn every_pre_handshake_request_gets_its_own_kind_of_answer() {
    let mut s = serve(None).await;
    let root = "/client/unit-ws".to_string();
    let WireMessage::StatusResponse(status) = s.ask(WireMessage::StatusRequest).await else {
        panic!("status");
    };
    assert_eq!(status.server_pid, std::process::id());
    let cluster = s.ask(WireMessage::ClusterRequest).await;
    assert!(
        matches!(cluster, WireMessage::ClusterResponse(_)),
        "{cluster:?}"
    );
    let place = PlaceRequest {
        workspace_name: "unit-ws".to_string(),
        engine: None,
        os: None,
        rebalance_active: false,
    };
    let answer = s.ask(WireMessage::PlaceRequest(place.clone())).await;
    assert!(
        matches!(answer, WireMessage::PlaceResponse(_)),
        "{answer:?}"
    );
    let answer = s.ask(WireMessage::PlacePreviewRequest(place)).await;
    assert!(
        matches!(answer, WireMessage::PlacePreviewResponse(_)),
        "{answer:?}"
    );
    let answer = s
        .ask(WireMessage::MetricsRequest(MetricsRequest {
            since_secs: 0,
        }))
        .await;
    assert!(
        matches!(answer, WireMessage::MetricsResponse(_)),
        "{answer:?}"
    );

    let answer = s
        .ask(WireMessage::ReadFileRequest(ReadFileRequest {
            path: "/no/such/file/for/unit/tests".to_string(),
            max_bytes: 0,
        }))
        .await;
    let WireMessage::ReadFileResponse(read) = answer else {
        panic!("{answer:?}")
    };
    assert!(read.content.is_none() && read.error.is_some(), "{read:?}");
    let answer = s
        .ask(WireMessage::PullFileRequest(PullFileRequest {
            client_workspace_root: root.clone(),
            base_workspace_name: Some("unit-ws".to_string()),
            path: "missing.bin".to_string(),
            offset: 0,
            list_directory: false,
        }))
        .await;
    assert!(
        matches!(answer, WireMessage::PullFileResponse(_)),
        "{answer:?}"
    );
    let answer = s
        .ask(WireMessage::SearchRequest(SearchRequest {
            client_workspace_root: root.clone(),
            base_workspace_name: Some("unit-ws".to_string()),
            query: "anything".to_string(),
            limit: 0,
            subpath: None,
            client_agent: None,
            client_host: None,
        }))
        .await;
    let WireMessage::SearchResponse(search) = answer else {
        panic!("{answer:?}")
    };
    assert!(search.hits.is_empty());

    let answer = s
        .ask(WireMessage::SyncRequest(SyncRequest {
            client_workspace_root: root.clone(),
            files: Vec::new(),
            clean_others: false,
            base_workspace_name: Some("unit-ws".to_string()),
        }))
        .await;
    let WireMessage::SyncResponse(sync) = answer else {
        panic!("{answer:?}")
    };
    assert_eq!((sync.files_updated, sync.files_deleted), (0, 0));
    let answer = s
        .ask(WireMessage::SyncProbeRequest(SyncProbeRequest {
            client_workspace_root: root.clone(),
            base_workspace_name: Some("unit-ws".to_string()),
            seed_from: None,
            files: Vec::new(),
        }))
        .await;
    let WireMessage::SyncProbeResponse(probe) = answer else {
        panic!("{answer:?}")
    };
    assert!(probe.missing.is_empty());

    // The workspace was created by the sync above, so a command now runs in it.
    let answer = s
        .ask(WireMessage::ExecRequest(ExecRequest {
            client_workspace_root: root.clone(),
            base_workspace_name: Some("unit-ws".to_string()),
            command: Vec::new(),
            env: Vec::new(),
            timeout_secs: 5,
            pull_changes: false,
            subdir: None,
            client_agent: None,
            client_host: None,
        }))
        .await;
    let WireMessage::ExecExit(exit) = answer else {
        panic!("{answer:?}")
    };
    assert_eq!(exit.error.as_deref(), Some("empty command"));
    let answer = s
        .ask(WireMessage::RemoteExecRequest(RemoteExecRequest {
            client_workspace_root: root,
            base_workspace_name: Some("unit-ws".to_string()),
            language: RemoteExecLanguage::Generic,
            command: RemoteExecCommand::Custom(String::new()),
            args: Vec::new(),
            env: Vec::new(),
            format: Default::default(),
            timeout_secs: 5,
            pull_changes: false,
            subdir: None,
            client_agent: None,
            client_host: None,
        }))
        .await;
    let WireMessage::RemoteExecResult(result) = answer else {
        panic!("{answer:?}")
    };
    assert_eq!(result.error.as_deref(), Some("empty command"));
    s.client
        .send(WireMessage::Disconnect {
            reason: "done".to_string(),
        })
        .await
        .unwrap();
    s.ends().await;
}
