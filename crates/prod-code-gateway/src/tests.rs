/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#![cfg(test)]

use super::*;

/// Only a server's requests are held back from the client, not its notifications (#391).
#[test]
fn a_server_request_is_told_from_a_notification() {
    assert!(super::is_server_request(
        r#"{"jsonrpc":"2.0","id":2,"method":"workspace/configuration","params":{}}"#
    ));
    assert!(!super::is_server_request(
        r#"{"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{"uri":"file:///a","diagnostics":[]}}"#
    ));
    assert!(!super::is_server_request(
        r#"{"jsonrpc":"2.0","id":2,"result":null}"#
    ));
}

use prod_code_protocol::{HostResources, PROTOCOL_VERSION};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

#[tokio::test]
async fn shared_output_queue_deadline_closes_every_generation_sender() {
    let (raw_tx, _rx) = rapidfire::mpsc::bounded(1);
    let output = SharedOutputSender::new(raw_tx, Duration::from_millis(10));
    output.send(WireMessage::Ping).await.unwrap();

    assert!(matches!(
        output.send(WireMessage::Ping).await,
        Err(SharedOutputSendError::Deadline)
    ));
    assert!(matches!(
        output.send(WireMessage::Ping).await,
        Err(SharedOutputSendError::Closed)
    ));
}

#[tokio::test]
async fn owned_join_aborts_and_observes_the_exact_writer_task() {
    let task = tokio::spawn(async {
        std::future::pending::<()>().await;
        Ok(())
    });
    let mut owned = OwnedJoin::new(task);
    owned.abort();
    let result = owned.task_mut().await;
    assert!(
        result
            .as_ref()
            .is_err_and(tokio::task::JoinError::is_cancelled)
    );
    assert!(flatten_writer_result(result).is_err());
    owned.clear_finished();
}

#[test]
fn read_server_file_caps_external_source_to_2mib_even_with_explicit_large_limit() {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return;
    };
    let external_base = home.join(".cargo/registry");
    if std::fs::create_dir_all(&external_base).is_err() {
        return;
    }
    let test_file = external_base.join(format!("test_cap_{}.txt", std::process::id()));
    let data = vec![b'x'; 3 * 1024 * 1024]; // 3 MiB
    if std::fs::write(&test_file, &data).is_err() {
        return;
    }
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let _guard = Cleanup(test_file.clone());

    let temp_storage = tempfile::tempdir().unwrap();

    let req = prod_code_protocol::ReadFileRequest {
        path: test_file.to_string_lossy().into_owned(),
        max_bytes: 64 * 1024 * 1024, // Explicit 64 MiB requested
    };
    let resp = read_server_file(temp_storage.path(), &req);

    assert!(
        resp.error.is_none(),
        "read_server_file failed: {:?}",
        resp.error
    );
    assert!(resp.truncated, "external source must be truncated to 2 MiB");
    let content = resp.content.expect("content present");
    assert_eq!(
        content.len(),
        2 * 1024 * 1024,
        "external source capped at 2 MiB"
    );
}

#[test]
fn read_server_file_allows_workspace_artifact_up_to_64mib() {
    let temp_storage = tempfile::tempdir().unwrap();
    let artifact = temp_storage.path().join("target/release/large_bin");
    std::fs::create_dir_all(artifact.parent().unwrap()).unwrap();
    let data = vec![b'y'; 3 * 1024 * 1024]; // 3 MiB
    std::fs::write(&artifact, &data).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&artifact, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let req = prod_code_protocol::ReadFileRequest {
        path: artifact.to_string_lossy().into_owned(),
        max_bytes: 0, // Default in workspace
    };
    let resp = read_server_file(temp_storage.path(), &req);
    assert!(
        resp.error.is_none(),
        "read_server_file failed: {:?}",
        resp.error
    );
    assert!(
        !resp.truncated,
        "workspace artifact must not be truncated under 64 MiB"
    );
    #[cfg(unix)]
    assert_eq!(
        resp.is_executable,
        Some(true),
        "gateway response must retain executable mode"
    );
    assert_eq!(resp.content.expect("content").len(), 3 * 1024 * 1024);
}

#[test]
fn is_readable_source_path_allows_polyglot_dependencies_and_rejects_arbitrary_files() {
    let storage = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    // 1. Rust cargo registry
    let cargo_file = home.path().join(".cargo/registry/src/github.com/lib.rs");
    std::fs::create_dir_all(cargo_file.parent().unwrap()).unwrap();
    std::fs::write(&cargo_file, "pub fn foo() {}").unwrap();
    assert!(is_readable_source_path_with_home(
        storage.path(),
        &cargo_file,
        Some(home.path())
    ));

    // 2. Python virtualenv / uv cache
    let py_file = home.path().join(".cache/uv/wheels/pkg/module.py");
    std::fs::create_dir_all(py_file.parent().unwrap()).unwrap();
    std::fs::write(&py_file, "def bar(): pass").unwrap();
    assert!(is_readable_source_path_with_home(
        storage.path(),
        &py_file,
        Some(home.path())
    ));

    // 3. Node pnpm store
    let pnpm_file = home.path().join(".local/share/pnpm/store/pkg/index.d.ts");
    std::fs::create_dir_all(pnpm_file.parent().unwrap()).unwrap();
    std::fs::write(&pnpm_file, "export declare const x: number;").unwrap();
    assert!(is_readable_source_path_with_home(
        storage.path(),
        &pnpm_file,
        Some(home.path())
    ));

    // 4. Do not expose unrelated checkouts just because they contain node_modules.
    let nm_file = home.path().join("projects/foo/node_modules/bar/index.js");
    std::fs::create_dir_all(nm_file.parent().unwrap()).unwrap();
    std::fs::write(&nm_file, "module.exports = {};").unwrap();
    assert!(!is_readable_source_path_with_home(
        storage.path(),
        &nm_file,
        Some(home.path())
    ));

    let workspace_nm_file = storage.path().join("workspace/node_modules/bar/index.js");
    std::fs::create_dir_all(workspace_nm_file.parent().unwrap()).unwrap();
    std::fs::write(&workspace_nm_file, "module.exports = {};").unwrap();
    assert!(is_readable_source_path(storage.path(), &workspace_nm_file));

    // 5. Arbitrary sensitive files rejected
    let ssh_key = home.path().join(".ssh/id_rsa");
    std::fs::create_dir_all(ssh_key.parent().unwrap()).unwrap();
    std::fs::write(&ssh_key, "private-key-material").unwrap();
    assert!(!is_readable_source_path_with_home(
        storage.path(),
        &ssh_key,
        Some(home.path())
    ));

    let bashrc = home.path().join(".bashrc");
    std::fs::write(&bashrc, "export SECRET=1").unwrap();
    assert!(!is_readable_source_path_with_home(
        storage.path(),
        &bashrc,
        Some(home.path())
    ));
}

#[test]
fn gopath_source_policy_checks_each_configured_root() {
    let temp = tempfile::tempdir().unwrap();
    let first = temp.path().join("first");
    let second = temp.path().join("second");
    let source = second.join("pkg/mod/example.test/module@v1/source.go");
    std::fs::create_dir_all(source.parent().unwrap()).unwrap();
    std::fs::write(&source, "package module\n").unwrap();
    let gopath = std::env::join_paths([first.as_os_str(), second.as_os_str()]).unwrap();

    assert!(is_gopath_source_path(
        &source.canonicalize().unwrap(),
        Some(&gopath)
    ));
    assert!(!is_gopath_source_path(&source, Some(first.as_os_str())));
}

#[tokio::test]
async fn own_gossip_does_not_propagate_unconfirmed_transitive_peers() {
    let storage = tempfile::tempdir().unwrap();
    let state = ServerState::new(storage.path().to_path_buf());
    *state.advertise.write().await = "127.0.0.1:9400".into();

    // Absorb gossip from peer A that mentions a transitive dead peer B.
    let peer_a_gossip = NodeGossip {
        addr: "127.0.0.1:9401".into(),
        status: state.status().await,
        workspaces: Vec::new(),
        peers: vec!["127.0.0.1:9402".into()], // peer B (unconfirmed)
        sent_at_ms: 1000,
    };
    state.absorb_gossip(peer_a_gossip).await;

    // own_gossip should contain peer A (which is confirmed alive by absorb_gossip),
    // but must NOT propagate peer B (which has never been directly observed alive).
    let own = state.own_gossip().await;
    assert!(
        own.peers.contains(&"127.0.0.1:9401".to_string()),
        "confirmed peer A must be gossiped"
    );
    assert!(
        !own.peers.contains(&"127.0.0.1:9402".to_string()),
        "unconfirmed peer B must NOT be propagated transitively"
    );
}

#[test]
fn shadow_root_ownership_follows_the_last_server_state_reference() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("shadow");
    let mut state = ServerState::new(fixture.path().join("storage"));
    state.shadow_root = root.clone();
    state._shadow_root_owner = Some(shadow::ShadowRootOwner::acquire(&root).unwrap());

    let accept_state = Arc::new(state);
    let session_state = Arc::clone(&accept_state);
    drop(accept_state);

    let refused = shadow::ShadowRootOwner::acquire(&root)
        .err()
        .expect("a retained session state must keep ownership");
    assert!(
        refused
            .to_string()
            .contains("already owned by another gateway"),
        "{refused:#}"
    );

    drop(session_state);
    let mut acquired = shadow::ShadowRootOwner::acquire(&root);
    for _ in 0..20 {
        if acquired.is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        acquired = shadow::ShadowRootOwner::acquire(&root);
    }
    acquired.expect("the final state release must release ownership");
}

#[tokio::test]
async fn incompatible_protocol_offers_are_refused_before_session_or_workspace_side_effects() {
    for (name, protocol_version, supported_versions) in [
        ("empty", PROTOCOL_VERSION, Some(Vec::new())),
        ("disjoint", PROTOCOL_VERSION, Some(vec![2, 3])),
        ("legacy-unsupported", 999, None),
    ] {
        let storage = tempfile::tempdir().unwrap();
        let storage_root = storage.path().join("workspaces");
        let state = Arc::new(ServerState::new(storage_root.clone()));
        let client_root = format!("/home/dev/{name}");
        let server_root = workspace::server_workspace_path(&storage_root, &client_root, None);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let serving = Arc::clone(&state);
        let serve = tokio::spawn(async move {
            let (socket, peer) = listener.accept().await.unwrap();
            handle_client(socket, peer, serving).await
        });

        let mut stream = TcpStream::connect(addr).await.unwrap();
        let mut payload = serde_json::json!({
            "protocol_version": protocol_version,
            "client_name": format!("{name}-offer-test"),
            "client_pid": 1,
            "auth_token": null,
            "client_workspace_root": client_root
        });
        if let Some(versions) = supported_versions {
            payload["supported_versions"] = serde_json::json!(versions);
        }
        let request = serde_json::json!({
            "type": "HandshakeRequest",
            "payload": payload
        });
        let bytes = serde_json::to_vec(&request).unwrap();
        stream
            .write_all(&(bytes.len() as u32).to_be_bytes())
            .await
            .unwrap();
        stream.write_all(&bytes).await.unwrap();
        let mut framed = Framed::new(stream, ProdCodeCodec::new());

        let Some(Ok(WireMessage::Disconnect { reason })) = framed.next().await else {
            panic!("the {name} protocol offer was not refused");
        };
        assert!(reason.contains("protocol"), "{reason}");
        drop(framed);
        serve.await.unwrap().unwrap();
        assert_eq!(state.next_session_id.load(Ordering::Relaxed), 1, "{name}");
        assert_eq!(state.active_sessions.load(Ordering::Relaxed), 0, "{name}");
        assert_eq!(state.workspace_manager.loaded_count().await, 0, "{name}");
        assert!(!server_root.exists(), "{name}");
    }
}

#[test]
fn native_position_coordinates_are_checked_before_one_based_conversion() {
    assert_eq!(
        one_based_position(Some(&serde_json::json!({ "line": 0, "character": 0 }))),
        Ok((1, 1))
    );
    assert_eq!(
        one_based_position(Some(&serde_json::json!({
            "line": 4_294_967_294u64,
            "character": 4_294_967_294u64,
        }))),
        Ok((u32::MAX, u32::MAX))
    );

    for malformed in [
        serde_json::json!({}),
        serde_json::json!({ "line": -1, "character": 0 }),
        serde_json::json!({ "line": 0.5, "character": 0 }),
        serde_json::json!({ "line": "0", "character": 0 }),
        serde_json::json!({ "line": null, "character": 0 }),
        serde_json::json!({ "line": 4_294_967_295u64, "character": 0 }),
    ] {
        assert!(one_based_position(Some(&malformed)).is_err(), "{malformed}");
    }
}

#[test]
fn native_methods_validate_positions_without_breaking_positionless_requests() {
    let point = serde_json::json!({ "line": 0, "character": 0 });
    assert!(
        native_position_params(
            Some("textDocument/hover"),
            Some(&serde_json::json!({ "position": point }))
        )
        .is_ok()
    );
    assert!(
        native_position_params(Some("textDocument/hover"), Some(&serde_json::json!({}))).is_err()
    );
    assert!(
        native_position_params(
            Some("callHierarchy/incomingCalls"),
            Some(&serde_json::json!({
                "item": { "selectionRange": { "start": { "line": -1, "character": 0 } } }
            }))
        )
        .is_err()
    );
    assert!(
        native_position_params(
            Some("prodCode/assists"),
            Some(&serde_json::json!({
                "range": { "start": { "line": 0, "character": 0 } }
            }))
        )
        .is_ok()
    );
    assert!(
        native_position_params(
            Some("prodCode/assists"),
            Some(&serde_json::json!({
                "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": 0, "character": "bad" }
                }
            }))
        )
        .is_err()
    );
    assert!(
        native_position_params(
            Some("prodCode/structuralReplace"),
            Some(&serde_json::json!({}))
        )
        .is_ok()
    );
    assert!(
        native_position_params(
            Some("prodCode/structuralReplace"),
            Some(&serde_json::json!({ "position": null }))
        )
        .is_err()
    );
    assert!(
        native_position_params(
            Some("textDocument/documentSymbol"),
            Some(&serde_json::json!({}))
        )
        .is_ok()
    );
    assert!(native_position_params(Some("workspace/symbol"), Some(&serde_json::json!({}))).is_ok());
    assert!(
        native_position_params(
            Some("textDocument/diagnostic"),
            Some(&serde_json::json!({}))
        )
        .is_ok()
    );
}

fn peer(addr: &str, platform: &str, engines: &[&str], load_per_cpu: f64) -> PeerInfo {
    let cpus = 8usize;
    PeerInfo {
        addr: addr.to_string(),
        status: StatusResponse {
            server_pid: 1,
            uptime_seconds: 1,
            active_sessions: 0,
            loaded_workspaces: 0,
            detected_engines: engines.iter().map(|e| e.to_string()).collect(),
            memory_rss_bytes: None,
            total_queries: 0,
            active_queries: 0,
            load_average_millis: Some((load_per_cpu * cpus as f64 * 1000.0) as u32),
            cpu_count: Some(cpus),
            platform: Some(platform.to_string()),
            running_commands: Vec::new(),
            host: Default::default(),
            version: None,
            git_commit: None,
        },
        workspaces: Vec::new(),
        last_seen_secs: 0,
        alive: true,
    }
}

/// A node past 85% of its memory or under 10% of its disk gets no new workspace while
/// another capable node has room, however quiet it is, and gives up an idle one it holds;
/// one with a session stays, and when every node is short the quietest still takes it
/// (#396).
#[test]
fn a_node_short_of_memory_or_disk_takes_no_new_workspace() {
    let gib = 1 << 30;
    let short_of_disk = HostResources {
        memory_available_bytes: Some(50 * gib),
        memory_total_bytes: Some(100 * gib),
        storage_free_millis: Some(30),
    };
    let short_of_memory = HostResources {
        memory_available_bytes: Some(5 * gib),
        storage_free_millis: Some(500),
        ..short_of_disk.clone()
    };
    let roomy = HostResources {
        storage_free_millis: Some(500),
        ..short_of_disk.clone()
    };
    let mut quiet_but_full = peer("full:9400", "linux x86_64", &["rust (ra_ap_ide)"], 0.05);
    quiet_but_full.status.host = short_of_disk;
    let mut busier = peer("busy:9400", "linux x86_64", &["rust (ra_ap_ide)"], 0.6);
    busier.status.host = roomy;
    let view = ClusterResponse {
        this_node: "full:9400".to_string(),
        nodes: vec![quiet_but_full, busier],
    };
    let place = |view: &ClusterResponse| {
        place_in(
            &PlaceRequest {
                workspace_name: "subject".to_string(),
                engine: Some("rust".to_string()),
                os: None,
                rebalance_active: false,
            },
            view.clone(),
        )
    };

    let answer = place(&view);
    assert_eq!(
        answer.node.as_deref(),
        Some("busy:9400"),
        "{}",
        answer.reason
    );
    assert!(
        answer
            .reason
            .contains("passed over full:9400 (disk 3% free)"),
        "{}",
        answer.reason
    );

    let mut held = view.clone();
    held.nodes[0].workspaces.push(LoadedWorkspaceInfo {
        name: "subject".to_string(),
        engine: "rust".to_string(),
        sessions: 0,
    });
    let answer = place(&held);
    assert_eq!(
        answer.node.as_deref(),
        Some("busy:9400"),
        "{}",
        answer.reason
    );
    assert!(
        answer
            .reason
            .starts_with("moved from full:9400 (disk 3% free, idle)"),
        "{}",
        answer.reason
    );

    held.nodes[0].workspaces[0].sessions = 1;
    assert_eq!(
        place(&held).node.as_deref(),
        Some("full:9400"),
        "a workspace in use is not moved"
    );

    let rebalance_answer = place_in(
        &PlaceRequest {
            workspace_name: "subject".to_string(),
            engine: Some("rust".to_string()),
            os: None,
            rebalance_active: true,
        },
        held.clone(),
    );
    assert_eq!(
        rebalance_answer.node.as_deref(),
        Some("busy:9400"),
        "rebalance_active moves an active workspace off an overloaded node"
    );
    assert!(
        rebalance_answer.reason.contains("active, 1 sessions"),
        "reason explains session state: {}",
        rebalance_answer.reason
    );

    let mut all_short = view.clone();
    all_short.nodes[1].status.host = short_of_memory;
    let answer = place(&all_short);
    assert_eq!(
        answer.node.as_deref(),
        Some("full:9400"),
        "{}",
        answer.reason
    );
    assert!(
        answer.reason.contains("it is short too (disk 3% free)"),
        "{}",
        answer.reason
    );

    // A gateway too old to report its host is not taken for one that is short.
    let mut old = view.clone();
    old.nodes[1].status.host = HostResources::default();
    assert_eq!(place(&old).node.as_deref(), Some("busy:9400"));
}

#[test]
fn congested_node_rebalances_active_workspace_when_requested() {
    let gib = 1 << 30;
    let roomy = HostResources {
        memory_available_bytes: Some(50 * gib),
        memory_total_bytes: Some(100 * gib),
        storage_free_millis: Some(500),
    };
    let mut congested = peer("congested:9400", "linux x86_64", &["rust (ra_ap_ide)"], 1.5);
    congested.status.host = roomy.clone();
    congested.workspaces.push(LoadedWorkspaceInfo {
        name: "repo".to_string(),
        engine: "rust".to_string(),
        sessions: 2,
    });
    let mut quiet = peer("quiet:9400", "linux x86_64", &["rust (ra_ap_ide)"], 0.2);
    quiet.status.host = roomy;
    let view = ClusterResponse {
        this_node: "congested:9400".to_string(),
        nodes: vec![congested, quiet],
    };

    // Without rebalance_active, active sessions are not moved:
    let normal = place_in(
        &PlaceRequest {
            workspace_name: "repo".to_string(),
            engine: Some("rust".to_string()),
            os: None,
            rebalance_active: false,
        },
        view.clone(),
    );
    assert_eq!(normal.node.as_deref(), Some("congested:9400"));
    assert!(normal.reason.contains("already loaded"));

    // With rebalance_active, active sessions are moved to the quieter node:
    let rebalanced = place_in(
        &PlaceRequest {
            workspace_name: "repo".to_string(),
            engine: Some("rust".to_string()),
            os: None,
            rebalance_active: true,
        },
        view,
    );
    assert_eq!(rebalanced.node.as_deref(), Some("quiet:9400"));
    assert!(rebalanced.reason.contains("rebalanced from congested:9400"));
    assert!(rebalanced.reason.contains("active, 2 sessions"));
}

/// Idle engines go after `--idle-evict-secs`, or after five minutes while memory is short,
/// even when eviction is switched off (#396).
#[test]
fn a_host_short_of_memory_unloads_idle_engines_sooner() {
    assert_eq!(evict_after(1800, false), Some(Duration::from_secs(1800)));
    assert_eq!(evict_after(0, false), None);
    assert_eq!(evict_after(1800, true), Some(Duration::from_secs(300)));
    assert_eq!(evict_after(120, true), Some(Duration::from_secs(120)));
    assert_eq!(evict_after(0, true), Some(Duration::from_secs(300)));
}

/// A handshake that needs a new engine on a host without the memory for it is refused with
/// capacity as the reason and a way forward, and leaves no session counted (#433).
#[tokio::test]
async fn a_handshake_without_memory_for_its_engine_is_refused_for_capacity() {
    const GIB: u64 = 1 << 30;
    let storage = tempfile::tempdir().expect("tempdir");
    let mut state = ServerState::new(storage.path().join("workspaces"));
    state.workspace_manager = Arc::new(WorkspaceManager::with_admission(Arc::new(
        admission::Admission::with_probe(
            admission::scripted_probe(vec![(10 * GIB, 100 * GIB)]),
            2048,
            admission::LOAD_SETTLE,
        ),
    )));
    let client_root = "/home/dev/app";
    let server_root = workspace::resolve_server_workspace(&state.storage_root, client_root, None);
    std::fs::create_dir_all(&server_root).unwrap();
    let state = Arc::new(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let serving = Arc::clone(&state);
    let serve = tokio::spawn(async move {
        let (socket, peer) = listener.accept().await.unwrap();
        handle_client(socket, peer, serving).await
    });
    let stream = TcpStream::connect(addr).await.unwrap();
    let mut framed = Framed::new(stream, ProdCodeCodec::new());
    framed
        .send(WireMessage::HandshakeRequest(
            prod_code_protocol::HandshakeRequest {
                protocol_version: PROTOCOL_VERSION,
                supported_versions: Some(vec![PROTOCOL_VERSION]),
                capabilities: None,
                client_name: "test".to_string(),
                client_pid: 1,
                auth_token: None,
                client_workspace_root: client_root.to_string(),
                preferred_engine: None,
                base_workspace_name: None,
                engine_subpath: None,
                client_agent: None,
                client_host: None,
                purpose: None,
                redirect_count: 0,
            },
        ))
        .await
        .unwrap();
    let Some(Ok(WireMessage::Disconnect { reason })) = framed.next().await else {
        panic!("the handshake was not refused");
    };
    assert!(reason.starts_with("capacity: "), "{reason}");
    assert!(reason.contains("memory 90% used"), "{reason}");
    assert!(reason.contains("Retry in a few minutes"), "{reason}");
    assert!(reason.contains("another node"), "{reason}");
    serve.await.unwrap().unwrap();
    assert_eq!(state.active_sessions.load(Ordering::Relaxed), 0);
    assert_eq!(state.workspace_manager.loaded_count().await, 0);
}

/// The first answer `state` gives a connection that opens with `token`, if any, and asks for
/// the status.
async fn status_answer(state: Arc<ServerState>, token: Option<&str>) -> WireMessage {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let serve = tokio::spawn(async move {
        let (socket, peer) = listener.accept().await.unwrap();
        handle_client(socket, peer, state).await
    });
    let stream = prod_code_protocol::transport::connect_with(addr, token)
        .await
        .unwrap();
    let mut framed = Framed::new(stream, ProdCodeCodec::new());
    framed.send(WireMessage::StatusRequest).await.unwrap();
    let answer = framed.next().await.unwrap().unwrap();
    drop(framed);
    let _ = serve.await;
    answer
}

/// A gateway with a token closes every connection that does not open with it and says
/// why, and serves one that does; a gateway without one ignores a token it is sent (#402).
#[tokio::test]
async fn a_gateway_with_a_token_serves_only_connections_that_open_with_it() {
    let storage = tempfile::tempdir().unwrap();
    let mut guarded = ServerState::new(storage.path().join("guarded"));
    guarded.auth_token = Some("s3cret".to_string());
    let guarded = Arc::new(guarded);
    for token in [None, Some("wrong"), Some("s3cre")] {
        match status_answer(Arc::clone(&guarded), token).await {
            WireMessage::Disconnect { reason } => {
                assert!(reason.contains("PROD_CODE_AUTH_TOKEN"), "{reason}")
            }
            other => panic!("served with {token:?}: {other:?}"),
        }
    }
    assert!(matches!(
        status_answer(Arc::clone(&guarded), Some("s3cret")).await,
        WireMessage::StatusResponse(_)
    ));

    let open = Arc::new(ServerState::new(storage.path().join("open")));
    assert!(matches!(
        status_answer(Arc::clone(&open), Some("anything")).await,
        WireMessage::StatusResponse(_)
    ));
    assert!(matches!(
        status_answer(open, None).await,
        WireMessage::StatusResponse(_)
    ));
}

/// An editor is offered what the language server on the node offers, but always asked for
/// whole documents, which is what the gateway hands on; the Rust engine offers what it
/// answers (#310).
#[test]
fn an_editor_gets_the_servers_capabilities_and_sends_whole_documents() {
    let gopls = serde_json::json!({
        "textDocumentSync": { "openClose": true, "change": 2, "save": {} },
        "completionProvider": { "triggerCharacters": ["."] },
        "hoverProvider": true
    });
    let caps = editor_capabilities(Some(gopls), false);
    assert_eq!(
        caps["textDocumentSync"],
        serde_json::json!({ "openClose": true, "change": 1, "save": {} })
    );
    assert_eq!(
        caps["completionProvider"]["triggerCharacters"],
        serde_json::json!(["."])
    );
    assert_eq!(caps["hoverProvider"], true);

    let numeric = editor_capabilities(Some(serde_json::json!({ "textDocumentSync": 2 })), false);
    assert_eq!(
        numeric["textDocumentSync"],
        serde_json::json!({ "openClose": true, "change": 1 })
    );

    let rust = editor_capabilities(None, true);
    assert_eq!(rust["renameProvider"], true);
    assert_eq!(rust["callHierarchyProvider"], true);
    assert_eq!(rust["completionProvider"]["resolveProvider"], true);
    assert_eq!(rust["codeActionProvider"]["resolveProvider"], true);
    assert_eq!(rust["textDocumentSync"]["change"], 1);
    // Every method advertised for Rust is one the engine answers.
    for (capability, method) in [
        ("completionProvider", "textDocument/completion"),
        ("signatureHelpProvider", "textDocument/signatureHelp"),
        ("inlayHintProvider", "textDocument/inlayHint"),
        (
            "documentHighlightProvider",
            "textDocument/documentHighlight",
        ),
        ("codeActionProvider", "textDocument/codeAction"),
        ("documentFormattingProvider", "textDocument/formatting"),
    ] {
        assert!(rust.get(capability).is_some(), "{capability}");
        assert!(
            prod_code_engine_rust::editor::EDITOR_METHODS.contains(&method),
            "{method}"
        );
    }

    assert_eq!(
        editor_capabilities(None, false),
        serde_json::json!({ "textDocumentSync": { "openClose": true, "change": 1 } })
    );
}

/// A macOS node is a developer's Mac: it takes work that needs macOS, or that no other live
/// node serves, and nothing else, however quiet it is (#308).
#[test]
fn a_macos_node_takes_only_what_needs_macos_or_what_nothing_else_serves() {
    let view = ClusterResponse {
        this_node: "linux:9400".to_string(),
        nodes: vec![
            peer(
                "linux:9400",
                "linux x86_64",
                &["rust (ra_ap_ide)", "go (gopls)"],
                0.9,
            ),
            peer(
                "mac:9400",
                "macos aarch64",
                &["swift (sourcekit-lsp)", "go (gopls)"],
                0.01,
            ),
        ],
    };
    let place = |view: &ClusterResponse, engine: Option<&str>, os: Option<&str>| {
        place_in(
            &PlaceRequest {
                workspace_name: "subject".to_string(),
                engine: engine.map(str::to_string),
                os: os.map(str::to_string),
                rebalance_active: false,
            },
            view.clone(),
        )
        .node
    };
    assert_eq!(
        place(&view, Some("go"), None).as_deref(),
        Some("linux:9400"),
        "plain Go stays on Linux though the Mac is far quieter"
    );
    assert_eq!(
        place(&view, None, None).as_deref(),
        Some("linux:9400"),
        "so does a workspace whose engine is unknown"
    );
    assert_eq!(
        place(&view, Some("go"), Some("macos")).as_deref(),
        Some("mac:9400"),
        "Go with macOS-only cgo goes to the Mac"
    );
    assert_eq!(
        place(&view, Some("swift"), None).as_deref(),
        Some("mac:9400"),
        "only the Mac serves Swift"
    );

    // A workspace already on the Mac that does not need macOS moves to Linux.
    let mut held = view.clone();
    held.nodes[1].workspaces.push(LoadedWorkspaceInfo {
        name: "subject".to_string(),
        engine: "go".to_string(),
        sessions: 0,
    });
    assert_eq!(
        place(&held, Some("go"), None).as_deref(),
        Some("linux:9400")
    );

    // With the Linux node down, the Mac takes plain Go rather than nothing.
    let mut down = view.clone();
    down.nodes[0].alive = false;
    assert_eq!(place(&down, Some("go"), None).as_deref(), Some("mac:9400"));
}

/// A new worktree's copy takes the seed's compiled crates, build-script outputs and
/// fingerprints with their modification times, and not its incremental caches (#278).
#[test]
fn a_seeded_copy_takes_the_build_cache_with_its_times() {
    let dir = tempfile::tempdir().unwrap();
    let (seed, fresh) = (dir.path().join("seed"), dir.path().join("fresh"));
    let debug = seed.join("target/debug");
    for (rel, text) in [
        ("deps/libdep-1a.rlib", "rlib"),
        ("build/dep-2b/out/generated.rs", "pub const X: u8 = 1;"),
        (".fingerprint/dep-1a/lib-dep", "fingerprint"),
        ("incremental/shop-3c/s-1/query-cache.bin", "incremental"),
    ] {
        let path = debug.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
    }
    let old = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
    std::fs::File::options()
        .write(true)
        .open(debug.join("deps/libdep-1a.rlib"))
        .unwrap()
        .set_modified(old)
        .unwrap();

    let copied = seed_build_cache_within(&seed, &fresh, roomy()).unwrap();
    assert_eq!(copied, Some(4 + 20 + 11));
    let out = fresh.join("target/debug");
    assert_eq!(
        std::fs::read_to_string(out.join("build/dep-2b/out/generated.rs")).unwrap(),
        "pub const X: u8 = 1;"
    );
    assert!(out.join(".fingerprint/dep-1a/lib-dep").is_file());
    assert!(!out.join("incremental").exists());
    let modified = std::fs::metadata(out.join("deps/libdep-1a.rlib"))
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(
        modified, old,
        "cargo compares these times; the copy must keep them"
    );
}

/// A new worktree's copy takes the seed's `node_modules` trees, the root's and a workspace
/// package's, with their symlinks kept as symlinks, and none from under `target` or `.git`;
/// without room for two of them it takes none (#412).
#[test]
fn a_seeded_copy_takes_the_node_modules_trees() {
    let dir = tempfile::tempdir().unwrap();
    let (seed, fresh) = (dir.path().join("seed"), dir.path().join("fresh"));
    for (rel, text) in [
        (
            "node_modules/zod/index.d.ts",
            "export declare const z: unknown;",
        ),
        ("node_modules/typescript/bin/tsc", "#!/usr/bin/env node"),
        ("node_modules/zod/node_modules/inner/index.js", "nested"),
        ("packages/app/node_modules/left-pad/index.js", "pad"),
        ("target/node_modules/stray.js", "not a package tree"),
        ("src/index.ts", "import { z } from 'zod';"),
    ] {
        let path = seed.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
    }
    std::fs::create_dir_all(seed.join("node_modules/.bin")).unwrap();
    std::os::unix::fs::symlink("../typescript/bin/tsc", seed.join("node_modules/.bin/tsc"))
        .unwrap();

    assert_eq!(
        dependency_trees(&seed),
        vec![
            PathBuf::from("node_modules"),
            PathBuf::from("packages/app/node_modules")
        ]
    );
    assert_eq!(
        seed_dependency_trees_within(&seed, &fresh, space(10, 1 << 40)).unwrap(),
        None,
        "no room for two of them"
    );
    assert!(!fresh.join("node_modules").exists());

    let copied = seed_dependency_trees_within(&seed, &fresh, roomy()).unwrap();
    assert_eq!(copied, Some(32 + 19 + 6 + 3));
    assert_eq!(
        std::fs::read_to_string(fresh.join("node_modules/zod/index.d.ts")).unwrap(),
        "export declare const z: unknown;"
    );
    assert!(
        fresh
            .join("node_modules/zod/node_modules/inner/index.js")
            .is_file()
    );
    assert!(
        fresh
            .join("packages/app/node_modules/left-pad/index.js")
            .is_file()
    );
    let link = fresh.join("node_modules/.bin/tsc");
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        std::fs::read_link(&link).unwrap(),
        PathBuf::from("../typescript/bin/tsc")
    );
    assert!(!fresh.join("target").exists());
    assert!(!fresh.join("src").exists(), "sources are copy_tree's");

    let bare = dir.path().join("bare");
    std::fs::create_dir_all(&bare).unwrap();
    assert_eq!(
        seed_dependency_trees_within(&bare, &dir.path().join("fresh2"), roomy()).unwrap(),
        None
    );
}

/// A virtual environment reaches the new copy with its symlinks kept (`lib64 -> lib` is not a
/// second copy of site-packages) and its scripts naming the copy; `copy_tree` leaves it to
/// the seeding of dependency trees, and keeps a directory symlink as a symlink instead of
/// walking it (#414).
#[test]
fn a_seeded_copy_takes_a_virtualenv_with_its_links_and_its_paths_rewritten() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let (seed, fresh) = (dir.path().join("seed"), dir.path().join("fresh"));
    let (venv, copy) = (seed.join(".venv"), fresh.join(".venv"));
    let old = venv.to_str().unwrap().to_string();
    for (rel, text) in [
        ("pyvenv.cfg", "home = /usr/bin\n".to_string()),
        (
            "lib/python3.12/site-packages/pkg/__init__.py",
            "x = 1\n".to_string(),
        ),
        ("bin/pytest", format!("#!{old}/bin/python\nimport pytest\n")),
        (
            "bin/activate",
            format!("VIRTUAL_ENV='{old}'\nexport VIRTUAL_ENV\n"),
        ),
    ] {
        let path = venv.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
    }
    std::fs::set_permissions(
        venv.join("bin/pytest"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    std::os::unix::fs::symlink("lib", venv.join("lib64")).unwrap();
    std::os::unix::fs::symlink("/usr/bin/python3", venv.join("bin/python")).unwrap();
    std::fs::create_dir_all(seed.join("src")).unwrap();
    std::fs::write(seed.join("src/app.py"), "import pkg\n").unwrap();
    std::os::unix::fs::symlink(".", seed.join("src/again")).unwrap();

    assert_eq!(copy_tree(&seed, &fresh).unwrap(), 1, "only src/app.py");
    assert!(!copy.exists(), "the venv is not copy_tree's");
    let again = std::fs::symlink_metadata(fresh.join("src/again")).unwrap();
    assert!(again.file_type().is_symlink());

    assert_eq!(dependency_trees(&seed), vec![PathBuf::from(".venv")]);
    assert!(
        seed_dependency_trees_within(&seed, &fresh, roomy())
            .unwrap()
            .is_some()
    );
    let link = |rel: &str| std::fs::read_link(copy.join(rel)).unwrap();
    assert_eq!(link("lib64"), PathBuf::from("lib"));
    assert_eq!(link("bin/python"), PathBuf::from("/usr/bin/python3"));
    assert!(
        copy.join("lib/python3.12/site-packages/pkg/__init__.py")
            .is_file()
    );
    let new = copy.to_str().unwrap();
    let pytest = std::fs::read_to_string(copy.join("bin/pytest")).unwrap();
    assert_eq!(pytest, format!("#!{new}/bin/python\nimport pytest\n"));
    let mode = std::fs::metadata(copy.join("bin/pytest"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o755, "the script stays executable");
    let activate = std::fs::read_to_string(copy.join("bin/activate")).unwrap();
    assert!(
        activate.contains(&format!("VIRTUAL_ENV='{new}'")),
        "{activate}"
    );
    assert!(!activate.contains(&format!("'{old}'")), "{activate}");
}

/// A seeded copy takes the sources and none of the seed's per-node caches: not its CMake
/// `build/` with the seed's `CMakeCache.txt` and `compile_commands.json`, not clangd's index,
/// not SwiftPM's `.build` (#416).
#[test]
fn a_seeded_copy_takes_no_build_directory_holding_the_seeds_paths() {
    let dir = tempfile::tempdir().unwrap();
    let (seed, fresh) = (dir.path().join("seed"), dir.path().join("fresh"));
    for rel in [
        "CMakeLists.txt",
        "src/main.cpp",
        "build/CMakeCache.txt",
        "build/compile_commands.json",
        ".cache/clangd/index/main.cpp.1A2B.idx",
        "lib/.build/debug.yaml",
        "tests/__pycache__/test_a.cpython-312.pyc",
    ] {
        let path = seed.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, rel).unwrap();
    }
    assert_eq!(copy_tree(&seed, &fresh).unwrap(), 2);
    assert!(fresh.join("CMakeLists.txt").is_file());
    assert!(fresh.join("src/main.cpp").is_file());
    for cache in ["build", ".cache", "lib/.build", "tests/__pycache__"] {
        assert!(!fresh.join(cache).exists(), "{cache} was copied");
    }
}

/// Room for a seed with plenty to spare.
fn roomy() -> Option<DiskSpace> {
    space(u64::MAX / 4, u64::MAX / 2)
}

fn space(free: u64, total: u64) -> Option<DiskSpace> {
    Some(DiskSpace { free, total })
}

/// No build cache, not enough room for two of it, or a copy that would leave less than a
/// fifth of the filesystem free, leaves the new copy without one (#419).
#[test]
fn a_seeded_copy_goes_without_a_build_cache_it_has_no_room_for() {
    let dir = tempfile::tempdir().unwrap();
    let (seed, fresh) = (dir.path().join("seed"), dir.path().join("fresh"));
    assert_eq!(
        seed_build_cache_within(&seed, &fresh, roomy()).unwrap(),
        None
    );
    let deps = seed.join("target/debug/deps");
    std::fs::create_dir_all(&deps).unwrap();
    std::fs::write(deps.join("libbig.rlib"), vec![0u8; 1000]).unwrap();
    assert_eq!(
        seed_build_cache_within(&seed, &fresh, space(1999, 5000)).unwrap(),
        None,
        "not twice its size free"
    );
    assert_eq!(seed_build_cache_within(&seed, &fresh, None).unwrap(), None);
    assert_eq!(
        seed_build_cache_within(&seed, &fresh, space(10_000, 46_000)).unwrap(),
        None,
        "9,000 left of 46,000 is under a fifth"
    );
    assert!(!fresh.join("target").exists());
    assert_eq!(
        seed_build_cache_within(&seed, &fresh, space(10_000, 44_000)).unwrap(),
        Some(1000),
        "9,000 left of 44,000 is more than a fifth"
    );
    assert!(
        disk_space(dir.path()).is_some_and(|s| s.free > 0 && s.total >= s.free),
        "{:?}",
        disk_space(dir.path())
    );
    assert!(disk_space(&dir.path().join("not/yet/created")).is_some());
}

/// A running command is in the status for as long as its handler runs, and gone however the
/// handler returns (#273).
#[test]
fn a_running_command_is_listed_until_its_handler_returns() {
    let workspace = Path::new("/srv/workspaces/shop--wt-status-test");
    let mine = |list: &[prod_code_protocol::RunningCommand]| {
        list.iter()
            .filter(|c| c.workspace == "shop--wt-status-test")
            .count()
    };
    let entry = RunningEntry::start(workspace, &["cargo".to_string(), "test".to_string()]);
    let listed = running_commands();
    assert_eq!(mine(&listed), 1);
    let command = listed
        .iter()
        .find(|c| c.workspace == "shop--wt-status-test")
        .unwrap();
    assert_eq!(command.command, "cargo test");
    drop(entry);
    assert_eq!(mine(&running_commands()), 0);
}

#[test]
fn a_compiler_cache_is_shared_across_worktrees_when_the_node_has_ccache() {
    let workspace = Path::new("/srv/workspaces/shop--wt-1a2b");
    assert!(compiler_cache_env(workspace, false).is_empty());
    let env = compiler_cache_env(workspace, true);
    let get = |k: &str| {
        env.iter()
            .find(|(key, _)| key == k)
            .map(|(_, v)| v.as_str())
    };
    assert_eq!(get("CCACHE_BASEDIR"), Some("/srv/workspaces/shop--wt-1a2b"));
    assert_eq!(get("CCACHE_NOHASHDIR"), Some("1"));
    assert_eq!(get("CCACHE_SLOPPINESS"), Some("pch_defines,time_macros"));
    assert_eq!(get("CCACHE_PCH_EXTSUM"), Some("1"));
    assert_eq!(get("CMAKE_C_COMPILER_LAUNCHER"), Some("ccache"));
    assert_eq!(get("CMAKE_CXX_COMPILER_LAUNCHER"), Some("ccache"));
    assert!(on_path("sh"), "sh is on PATH on every node");
    assert!(!on_path("no-such-program-on-any-node"));
}

/// A directory renamed or deleted locally leaves nothing behind on the copy (#124): the files
/// the manifest no longer lists go, and so do the directories that held only them, while a
/// directory that still holds a file, the workspace root and the per-node caches stay.
#[test]
fn a_directory_gone_locally_is_gone_from_the_copy() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    for (rel, body) in [
        ("Sources/OldApp/main.swift", "old"),
        ("Sources/Unused/deep/x.swift", "unused"),
        ("Sources/NewApp/main.swift", "new"),
        ("Package.swift", "pkg"),
        (".build/debug/cache.o", "cache"),
    ] {
        std::fs::create_dir_all(root.join(rel).parent().unwrap()).unwrap();
        std::fs::write(root.join(rel), body).unwrap();
    }
    std::fs::create_dir_all(root.join("Sources/Empty/deeper")).unwrap();
    let stamp = |rel: &str, body: &str| FileStamp {
        relative_path: rel.to_string(),
        size: body.len() as u64,
        hash: content_hash(body.as_bytes()),
    };
    let manifest = [
        stamp("Sources/NewApp/main.swift", "new"),
        stamp("Package.swift", "pkg"),
    ];
    let (missing, mut deleted) = reconcile_manifest(root, &manifest);
    deleted.sort();
    assert!(missing.is_empty(), "{missing:?}");
    assert_eq!(
        deleted,
        ["Sources/OldApp/main.swift", "Sources/Unused/deep/x.swift"]
    );
    for gone in ["Sources/OldApp", "Sources/Unused", "Sources/Empty"] {
        assert!(!root.join(gone).exists(), "{gone} is still on the copy");
    }
    assert!(root.join("Sources/NewApp/main.swift").is_file());
    assert!(
        root.join(".build/debug/cache.o").is_file(),
        "a node cache is not touched"
    );

    // A single deletion climbs only as far as the directories it empties.
    std::fs::create_dir_all(root.join("a/b/c")).unwrap();
    std::fs::write(root.join("a/keep.rs"), "k").unwrap();
    std::fs::write(root.join("a/b/c/gone.rs"), "g").unwrap();
    std::fs::remove_file(root.join("a/b/c/gone.rs")).unwrap();
    let emptied = root.join("a/b/c");
    prune_empty_parents(root, Some(&emptied));
    assert!(!root.join("a/b").exists());
    assert!(root.join("a/keep.rs").is_file());
    prune_empty_parents(root, Some(root));
    assert!(root.exists(), "the workspace root itself is never removed");
}

#[test]
fn test_changed_since_reports_new_changed_and_deleted() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("target/debug")).unwrap();
    std::fs::write(root.join("src/a.rs"), "a").unwrap();
    std::fs::write(root.join("src/gone.rs"), "g").unwrap();
    std::fs::write(root.join("Cargo.lock"), "l1").unwrap();
    let before = snapshot_tree(root);

    std::fs::write(root.join("src/a.rs"), "a formatted").unwrap();
    std::fs::write(root.join("src/new.rs"), "n").unwrap();
    std::fs::remove_file(root.join("src/gone.rs")).unwrap();
    std::fs::write(root.join("target/debug/junk.o"), "x").unwrap();

    let changed = changed_since(root, &before.stamps);
    let names: Vec<(&str, bool)> = changed
        .iter()
        .map(|f| (f.relative_path.as_str(), f.content.is_some()))
        .collect();
    assert_eq!(
        names,
        vec![
            ("src/a.rs", true),
            ("src/gone.rs", false),
            ("src/new.rs", true)
        ]
    );
    assert_eq!(
        changed[0].content.as_deref(),
        Some(b"a formatted".as_slice())
    );
}

/// A command whose client goes away mid-run leaves the copy exactly as it found it (#262):
/// the file it rewrote has its old text back, the files it created are gone with the
/// directory it made, and the file it deleted is there again, executable bit included.
#[tokio::test]
async fn a_command_whose_client_leaves_changes_nothing_in_the_copy() {
    let storage = tempfile::tempdir().unwrap();
    let workspace = storage.path().join("restore-ws");
    std::fs::create_dir_all(workspace.join("src")).unwrap();
    std::fs::write(workspace.join("src/a.rs"), "fn a() {}\n").unwrap();
    std::fs::write(workspace.join("src/gone.rs"), "fn gone() {}\n").unwrap();
    std::fs::write(workspace.join("run.sh"), "#!/bin/sh\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            workspace.join("run.sh"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
    let before = stamp_tree(&workspace);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    let (server, _) = listener.accept().await.unwrap();
    let storage_root = storage.path().to_path_buf();
    let metrics_dir = tempfile::tempdir().unwrap();
    let metrics = metrics::Metrics::new(metrics_dir.path().to_path_buf());
    let gateway = tokio::spawn(async move {
        let manager = WorkspaceManager::new();
        let mut framed = Framed::new(AnyStream::from(server), ProdCodeCodec::new());
        let req = ExecRequest {
            client_workspace_root: "/tmp/restore-ws".to_string(),
            base_workspace_name: Some("restore-ws".to_string()),
            command: [
                "sh",
                "-c",
                "printf 'fn a() { formatted }' > src/a.rs; printf new > src/new.rs; \
                     rm src/gone.rs run.sh; mkdir -p src/deep; printf x > src/deep/made.rs; \
                     echo ready; sleep 60",
            ]
            .map(str::to_string)
            .to_vec(),
            env: Vec::new(),
            timeout_secs: 120,
            pull_changes: true,
            subdir: None,
            client_agent: None,
            client_host: None,
        };
        run_exec(&storage_root, &metrics, &manager, &mut framed, req).await
    });

    let mut framed = Framed::new(client, ProdCodeCodec::new());
    let mut output = Vec::new();
    while !String::from_utf8_lossy(&output).contains("ready") {
        match tokio::time::timeout(std::time::Duration::from_secs(30), framed.next()).await {
            Ok(Some(Ok(WireMessage::ExecChunk(chunk)))) => {
                output.extend(chunk.data.unwrap_or_default())
            }
            other => panic!("no output from the command: {other:?}"),
        }
    }
    assert!(workspace.join("src/new.rs").is_file(), "the command ran");
    drop(framed);

    tokio::time::timeout(std::time::Duration::from_secs(30), gateway)
        .await
        .expect("the command was killed, not left to run out its sleep")
        .unwrap()
        .unwrap();
    assert_eq!(stamp_tree(&workspace), before);
    assert_eq!(
        std::fs::read_to_string(workspace.join("src/a.rs")).unwrap(),
        "fn a() {}\n"
    );
    assert!(!workspace.join("src/deep").exists());
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(workspace.join("run.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755);
    }
    assert!(workspace::stale_paths(&workspace).is_empty());
}

#[tokio::test]
async fn test_exec_fails_when_subdir_does_not_exist() {
    let storage = tempfile::tempdir().unwrap();
    let workspace = storage.path().join("test-ws");
    std::fs::create_dir_all(&workspace).unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    let (server, _) = listener.accept().await.unwrap();
    let storage_root = storage.path().to_path_buf();
    let metrics_dir = tempfile::tempdir().unwrap();
    let metrics = metrics::Metrics::new(metrics_dir.path().to_path_buf());
    let gateway = tokio::spawn(async move {
        let manager = WorkspaceManager::new();
        let mut framed = Framed::new(AnyStream::from(server), ProdCodeCodec::new());
        let req = ExecRequest {
            client_workspace_root: "/tmp/test-ws".to_string(),
            base_workspace_name: Some("test-ws".to_string()),
            command: vec!["pwd".to_string()],
            env: Vec::new(),
            timeout_secs: 10,
            pull_changes: false,
            subdir: Some("nonexistent_sub".to_string()),
            client_agent: None,
            client_host: None,
        };
        run_exec(&storage_root, &metrics, &manager, &mut framed, req).await
    });

    let mut framed = Framed::new(client, ProdCodeCodec::new());
    let exit = match tokio::time::timeout(std::time::Duration::from_secs(10), framed.next()).await {
        Ok(Some(Ok(WireMessage::ExecExit(exit)))) => exit,
        other => panic!("expected ExecExit, got: {other:?}"),
    };
    assert!(
        exit.error
            .as_deref()
            .unwrap_or_default()
            .contains("does not exist")
    );
    gateway.await.unwrap().unwrap();
}

/// A file that a client sync delivered while the command ran is the checkout's text and is
/// left alone; one the command changed again after it arrived is removed and reported stale;
/// one only the command changed gets its old bytes back (#262).
#[test]
fn a_restore_keeps_what_a_sync_delivered_during_the_command() {
    let storage = tempfile::tempdir().unwrap();
    let root = storage.path();
    for name in ["cmd.txt", "synced.txt", "both.txt"] {
        std::fs::write(root.join(name), "old\n").unwrap();
    }
    let before = snapshot_tree_within(root, RESTORE_MAX_FILE, RESTORE_BUDGET);
    std::fs::write(root.join("cmd.txt"), "the command's\n").unwrap();
    std::fs::write(root.join("synced.txt"), "the client's\n").unwrap();
    std::fs::write(root.join("both.txt"), "the command's, after the sync\n").unwrap();
    let synced = std::collections::HashMap::from([
        (
            "synced.txt".to_string(),
            Some(content_hash(b"the client's\n")),
        ),
        (
            "both.txt".to_string(),
            Some(content_hash(b"the client's\n")),
        ),
    ]);

    let restored = restore_tree(root, &before, &synced);

    assert_eq!(
        std::fs::read_to_string(root.join("cmd.txt")).unwrap(),
        "old\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("synced.txt")).unwrap(),
        "the client's\n"
    );
    assert!(!root.join("both.txt").exists());
    assert_eq!(restored.stale, vec!["both.txt".to_string()]);
    let mut files: Vec<&str> = restored
        .files
        .iter()
        .map(|f| f.relative_path.as_str())
        .collect();
    files.sort();
    assert_eq!(files, vec!["both.txt", "cmd.txt"]);
}

#[test]
fn a_sync_is_remembered_per_workspace_from_the_time_it_lands() {
    let one = std::path::Path::new("/nonexistent/sync-log-one");
    let other = std::path::Path::new("/nonexistent/sync-log-other");
    let start = Instant::now();
    workspace::record_synced(
        one,
        &[("a.rs".to_string(), Some(7)), ("gone.rs".to_string(), None)],
    );
    let synced = workspace::synced_since(one, start);
    assert_eq!(synced.get("a.rs"), Some(&Some(7)));
    assert_eq!(synced.get("gone.rs"), Some(&None));
    assert!(workspace::synced_since(other, start).is_empty());
    assert!(workspace::synced_since(one, Instant::now()).is_empty());
    workspace::record_synced(one, &[]);
}

/// A file whose old bytes did not fit the snapshot's limits cannot be put back: it leaves the
/// copy and is reported stale by every sync answer until the client has sent it (#262).
#[tokio::test]
async fn a_file_past_the_snapshot_limits_is_removed_and_reported_stale() {
    let storage = tempfile::tempdir().unwrap();
    let root = storage.path().join("ws");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.rs"), "aaaa").unwrap();
    std::fs::write(root.join("src/b.rs"), "bbbb").unwrap();
    std::fs::write(root.join("src/big.rs"), "0123456789").unwrap();
    // Files are kept in path order: a.rs fits, b.rs is over the budget a.rs leaves, and
    // big.rs is over the per-file limit.
    let before = snapshot_tree_within(&root, 8, 6);
    assert_eq!(before.kept.keys().collect::<Vec<_>>(), ["src/a.rs"]);

    std::fs::write(root.join("src/a.rs"), "changed").unwrap();
    std::fs::write(root.join("src/b.rs"), "changed").unwrap();
    std::fs::remove_file(root.join("src/big.rs")).unwrap();
    let manager = WorkspaceManager::new();
    let restored =
        restore_after_lost_client(&manager, &root, Arc::new(before), Instant::now()).await;
    assert_eq!(restored, 1);
    assert_eq!(
        std::fs::read_to_string(root.join("src/a.rs")).unwrap(),
        "aaaa"
    );
    assert!(
        !root.join("src/b.rs").exists(),
        "a changed file not kept leaves"
    );
    assert_eq!(
        workspace::stale_paths(&root),
        ["src/b.rs".to_string(), "src/big.rs".to_string()]
    );

    // The next sync answers with what it still lacks until the client has sent both.
    let sync = |files: Vec<FileDelta>| SyncRequest {
        client_workspace_root: "/tmp/ws".to_string(),
        files,
        clean_others: false,
        base_workspace_name: Some("ws".to_string()),
    };
    let first = apply_sync(
        storage.path(),
        &manager,
        sync(vec![
            FileDelta {
                relative_path: "src/a.rs".to_string(),
                content: Some(b"aaaa".to_vec()),
                is_executable: false,
            },
            FileDelta {
                relative_path: "src/b.rs".to_string(),
                content: Some(b"bbbb".to_vec()),
                is_executable: false,
            },
        ]),
    )
    .await;
    assert_eq!(first.stale_paths, ["src/big.rs".to_string()]);
    let second = apply_sync(
        storage.path(),
        &manager,
        sync(vec![FileDelta {
            relative_path: "src/big.rs".to_string(),
            content: None,
            is_executable: false,
        }]),
    )
    .await;
    assert!(second.stale_paths.is_empty());
    assert!(!root.join(workspace::STALE_MARKER).exists());
}

/// A file the copy cannot take keeps its old text, is not counted, and comes back stale until
/// the client has sent it again. A read-only directory stands in for a full disk, where
/// `fs::write` truncated the file and reported nothing (#385).
#[tokio::test]
async fn a_sync_write_that_fails_keeps_the_old_text_and_asks_for_it_again() {
    use std::os::unix::fs::PermissionsExt;
    let storage = tempfile::tempdir().unwrap();
    let manager = WorkspaceManager::new();
    let sync = |text: &str| SyncRequest {
        client_workspace_root: "/tmp/ws".to_string(),
        files: vec![FileDelta {
            relative_path: "src/lib.rs".to_string(),
            content: Some(text.as_bytes().to_vec()),
            is_executable: false,
        }],
        clean_others: false,
        base_workspace_name: Some("ws".to_string()),
    };
    apply_sync(storage.path(), &manager, sync("fn old() {}")).await;
    let root = storage.path().join("ws");
    let src = root.join("src");
    std::fs::set_permissions(&src, std::fs::Permissions::from_mode(0o555)).unwrap();
    let refused = apply_sync(storage.path(), &manager, sync("fn new() {}")).await;
    std::fs::set_permissions(&src, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        std::fs::read_to_string(src.join("lib.rs")).unwrap(),
        "fn old() {}"
    );
    assert_eq!(refused.files_updated, 0);
    assert_eq!(refused.stale_paths, ["src/lib.rs".to_string()]);
    assert_eq!(workspace::stale_paths(&root), ["src/lib.rs".to_string()]);
    assert!(
        std::fs::read_dir(&src).unwrap().count() == 1,
        "no temporary file is left behind"
    );

    let again = apply_sync(storage.path(), &manager, sync("fn new() {}")).await;
    assert_eq!(again.files_updated, 1);
    assert!(again.stale_paths.is_empty());
    assert_eq!(
        std::fs::read_to_string(src.join("lib.rs")).unwrap(),
        "fn new() {}"
    );
}

#[tokio::test]
async fn test_delta_sync_reports_fresh_until_probed() {
    let storage = tempfile::tempdir().unwrap();
    let manager = WorkspaceManager::new();
    let delta = || SyncRequest {
        client_workspace_root: "/tmp/ws".to_string(),
        files: vec![FileDelta {
            relative_path: "src/lib.rs".to_string(),
            content: Some(b"fn a() {}".to_vec()),
            is_executable: false,
        }],
        clean_others: false,
        base_workspace_name: Some("ws".to_string()),
    };
    let first = apply_sync(storage.path(), &manager, delta()).await;
    assert!(
        first.workspace_was_fresh,
        "nobody established this workspace yet"
    );
    let probe = apply_sync_probe(
        storage.path(),
        &manager,
        SyncProbeRequest {
            client_workspace_root: "/tmp/ws".to_string(),
            base_workspace_name: Some("ws".to_string()),
            seed_from: None,
            files: vec![FileStamp {
                relative_path: "src/lib.rs".to_string(),
                size: 9,
                hash: content_hash(b"fn a() {}"),
            }],
        },
    )
    .await;
    assert!(probe.missing.is_empty());
    let second = apply_sync(storage.path(), &manager, delta()).await;
    assert!(
        !second.workspace_was_fresh,
        "probe established the workspace"
    );
}

#[tokio::test]
async fn test_sync_probe_seeds_and_reconciles() {
    let storage = tempfile::tempdir().unwrap();
    let origin = storage.path().join("repo");
    std::fs::create_dir_all(origin.join("src")).unwrap();
    std::fs::write(origin.join("Cargo.toml"), "[package]\nname = \"repo\"\n").unwrap();
    std::fs::write(origin.join("src/lib.rs"), "pub fn a() {}").unwrap();
    std::fs::write(origin.join("src/only_in_origin.rs"), "pub fn gone() {}").unwrap();
    let manager = WorkspaceManager::new();

    let req = SyncProbeRequest {
        client_workspace_root: "/tmp/wt".to_string(),
        base_workspace_name: Some("repo--wt-0001".to_string()),
        seed_from: Some("repo".to_string()),
        files: vec![
            FileStamp {
                relative_path: "Cargo.toml".to_string(),
                size: 24,
                hash: content_hash(b"[package]\nname = \"repo\"\n"),
            },
            FileStamp {
                relative_path: "src/lib.rs".to_string(),
                size: 21,
                hash: content_hash(b"pub fn a() -> u8 {}"),
            },
            FileStamp {
                relative_path: "src/new.rs".to_string(),
                size: 3,
                hash: content_hash(b"// n"),
            },
        ],
    };
    let resp = apply_sync_probe(storage.path(), &manager, req).await;
    assert!(resp.seeded);
    assert_eq!(resp.files_deleted, 1);
    assert_eq!(
        resp.missing,
        vec!["src/lib.rs".to_string(), "src/new.rs".to_string()]
    );
    let wt = storage.path().join("repo--wt-0001");
    assert!(wt.join("Cargo.toml").exists());
    assert!(!wt.join("src/only_in_origin.rs").exists());
    assert!(
        origin.join("src/only_in_origin.rs").exists(),
        "origin copy untouched"
    );

    // A second probe on the now-populated directory does not seed again.
    let again = apply_sync_probe(
        storage.path(),
        &manager,
        SyncProbeRequest {
            client_workspace_root: "/tmp/wt".to_string(),
            base_workspace_name: Some("repo--wt-0001".to_string()),
            seed_from: Some("repo".to_string()),
            files: vec![FileStamp {
                relative_path: "Cargo.toml".to_string(),
                size: 24,
                hash: content_hash(b"[package]\nname = \"repo\"\n"),
            }],
        },
    )
    .await;
    assert!(!again.seeded);
    assert!(again.missing.is_empty());
    assert_eq!(
        again.files_deleted, 1,
        "src/lib.rs is not in the manifest any more"
    );
}

#[tokio::test]
async fn test_server_state_status() {
    let temp = tempfile::tempdir().unwrap();
    let state = ServerState::new(temp.path().to_path_buf());
    let status = state.status().await;
    assert_eq!(status.server_pid, std::process::id());
    assert_eq!(status.active_sessions, 0);
    assert_eq!(status.loaded_workspaces, 0);
    assert!(
        status
            .detected_engines
            .contains(&"rust (ra_ap_ide)".to_string())
    );
    assert!(state.serves_engine("rust") && state.serves_engine("swift"));
}

#[tokio::test]
async fn test_engine_allowlist_narrows_advertised_engines() {
    let temp = tempfile::tempdir().unwrap();
    let mut state = ServerState::new(temp.path().to_path_buf());
    state.engine_allowlist = vec!["swift".to_string()];
    let status = state.status().await;
    assert!(
        !status
            .detected_engines
            .iter()
            .any(|e| e.starts_with("rust") || e == "generic-lsp"),
        "{:?}",
        status.detected_engines
    );
    assert!(
        status
            .detected_engines
            .iter()
            .all(|e| e.starts_with("swift"))
    );
    assert!(state.serves_engine("swift") && state.serves_engine("Swift"));
    assert!(!state.serves_engine("rust") && !state.serves_engine("generic"));
}

#[test]
fn test_engine_detection() {
    use prod_code_protocol::messages::EngineKind;

    let temp = tempfile::tempdir().unwrap();
    assert_eq!(detect_engine(temp.path()), EngineKind::Generic);

    std::fs::write(temp.path().join("Cargo.toml"), "").unwrap();
    assert_eq!(detect_engine(temp.path()), EngineKind::Rust);

    let go_temp = tempfile::tempdir().unwrap();
    std::fs::write(go_temp.path().join("go.mod"), "").unwrap();
    assert_eq!(detect_engine(go_temp.path()), EngineKind::Go);

    let py_temp = tempfile::tempdir().unwrap();
    std::fs::write(py_temp.path().join("pyproject.toml"), "").unwrap();
    assert_eq!(detect_engine(py_temp.path()), EngineKind::Python);

    let ts_temp = tempfile::tempdir().unwrap();
    std::fs::write(ts_temp.path().join("package.json"), "").unwrap();
    assert_eq!(detect_engine(ts_temp.path()), EngineKind::TypeScript);

    let java_temp = tempfile::tempdir().unwrap();
    std::fs::write(java_temp.path().join("pom.xml"), "").unwrap();
    assert_eq!(detect_engine(java_temp.path()), EngineKind::Java);

    let kt_temp = tempfile::tempdir().unwrap();
    std::fs::write(
        kt_temp.path().join("build.gradle.kts"),
        "plugins { kotlin(\"jvm\") }",
    )
    .unwrap();
    assert_eq!(detect_engine(kt_temp.path()), EngineKind::Kotlin);

    let cs_temp = tempfile::tempdir().unwrap();
    std::fs::write(cs_temp.path().join("App.csproj"), "").unwrap();
    assert_eq!(detect_engine(cs_temp.path()), EngineKind::Csharp);

    let php_temp = tempfile::tempdir().unwrap();
    std::fs::write(php_temp.path().join("composer.json"), "").unwrap();
    assert_eq!(detect_engine(php_temp.path()), EngineKind::Php);

    let rb_temp = tempfile::tempdir().unwrap();
    std::fs::write(rb_temp.path().join("Gemfile"), "").unwrap();
    assert_eq!(detect_engine(rb_temp.path()), EngineKind::Ruby);
}

#[tokio::test]
async fn test_apply_sync_create_and_delete() {
    use prod_code_protocol::FileDelta;

    let storage_temp = tempfile::tempdir().unwrap();
    let client_root = "/Users/testuser/Projects/my-app";

    let req = SyncRequest {
        client_workspace_root: client_root.to_string(),
        files: vec![
            FileDelta {
                relative_path: "src/lib.rs".to_string(),
                content: Some(b"pub fn add(a: i32, b: i32) -> i32 { a + b }".to_vec()),
                is_executable: false,
            },
            FileDelta {
                relative_path: "README.md".to_string(),
                content: Some(b"# My App".to_vec()),
                is_executable: false,
            },
        ],
        clean_others: false,
        base_workspace_name: None,
    };

    let resp = apply_sync(storage_temp.path(), &WorkspaceManager::new(), req).await;
    assert_eq!(resp.files_updated, 2);
    assert_eq!(resp.files_deleted, 0);

    let app_dir = storage_temp.path().join("my-app");
    assert!(app_dir.join("src/lib.rs").exists());
    assert!(app_dir.join("README.md").exists());
    let content = std::fs::read_to_string(app_dir.join("src/lib.rs")).unwrap();
    assert!(content.contains("pub fn add"));

    // Now test deleting README.md
    let del_req = SyncRequest {
        client_workspace_root: client_root.to_string(),
        files: vec![FileDelta {
            relative_path: "README.md".to_string(),
            content: None,
            is_executable: false,
        }],
        clean_others: false,
        base_workspace_name: None,
    };

    let del_resp = apply_sync(storage_temp.path(), &WorkspaceManager::new(), del_req).await;
    assert_eq!(del_resp.files_updated, 0);
    assert_eq!(del_resp.files_deleted, 1);
    assert!(!app_dir.join("README.md").exists());
    assert!(app_dir.join("src/lib.rs").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn sync_rejects_absolute_parent_and_symlink_paths() {
    use std::os::unix::fs::symlink;

    let storage = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let manager = WorkspaceManager::new();
    let root = storage.path().join("ws");
    std::fs::create_dir_all(&root).unwrap();
    symlink(outside.path(), root.join("linked")).unwrap();

    let victim = outside.path().join("victim.txt");
    std::fs::write(&victim, b"keep").unwrap();
    let absolute_write = outside.path().join("absolute-escape.txt");
    let parent_write = outside.path().join("parent-escape.txt");
    let symlink_write = outside.path().join("symlink-escape.txt");
    let response = apply_sync(
        storage.path(),
        &manager,
        SyncRequest {
            client_workspace_root: "/tmp/ws".to_string(),
            files: vec![
                FileDelta {
                    relative_path: "../parent-escape.txt".to_string(),
                    content: Some(b"parent".to_vec()),
                    is_executable: false,
                },
                FileDelta {
                    relative_path: absolute_write.to_string_lossy().into_owned(),
                    content: Some(b"absolute".to_vec()),
                    is_executable: false,
                },
                FileDelta {
                    relative_path: "linked/symlink-escape.txt".to_string(),
                    content: Some(b"symlink".to_vec()),
                    is_executable: false,
                },
                FileDelta {
                    relative_path: "../victim.txt".to_string(),
                    content: None,
                    is_executable: false,
                },
            ],
            clean_others: false,
            base_workspace_name: Some("ws".to_string()),
        },
    )
    .await;

    assert_eq!(response.files_updated, 0);
    assert_eq!(response.files_deleted, 0);
    for rejected in [
        "../parent-escape.txt",
        absolute_write.to_str().unwrap(),
        "linked/symlink-escape.txt",
        "../victim.txt",
    ] {
        assert!(
            response.stale_paths.iter().any(|path| path == rejected),
            "invalid path should be retried: {rejected:?}; stale paths: {:?}",
            response.stale_paths
        );
    }
    assert!(!parent_write.exists());
    assert!(!absolute_write.exists());
    assert!(!symlink_write.exists());
    assert_eq!(std::fs::read(&victim).unwrap(), b"keep");
}

/// Both halves of the engine cache, in one test because the cache is process-global and
/// two tests would race for it. The sentinel is a value the probe cannot produce, so a
/// sentinel coming back proves the probe did not run, and a sentinel gone proves it did.
#[test]
fn engines_are_served_from_the_cache_until_it_expires() {
    let sentinel = vec!["sentinel (not a real engine)".to_string()];

    store_engines(Instant::now() + ENGINE_CACHE_TTL, sentinel.clone());
    assert_eq!(
        cached_available_engines(),
        sentinel,
        "a live entry must be answered without probing"
    );

    store_engines(Instant::now(), sentinel.clone());
    let fresh = cached_available_engines();
    assert_ne!(fresh, sentinel, "an expired entry must be probed again");
    assert!(
        fresh.iter().any(|e| e.starts_with("rust ")),
        "the probe always reports the in-process Rust engine, got {fresh:?}"
    );
    assert_eq!(
        cached_available_engines(),
        fresh,
        "the probe's answer is what the next caller gets"
    );
}

/// A refactoring's rewrites are read at the paths the files had before it, and LSP applies
/// `documentChanges` in order, so they go out before the moves. Sent after them, the rewrite
/// of `a.rs` would land on the file `c.rs` was just moved to. Applied by the client, a module
/// rename (`foo.rs` and `foo/`, with a file created inside it) lands whole.
#[test]
fn a_refactoring_is_serialized_with_its_rewrites_before_its_moves() {
    use prod_code_engine_rust::{FileMove, RefactorOutcome, RewrittenFile};
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let write = |rel: &str, text: &str| {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    write("src/lib.rs", "mod foo;\nmod a;\nmod c;\n");
    write("src/foo.rs", "mod inner;\npub use inner::f;\n");
    write("src/foo/inner.rs", "pub fn f() -> u8 { crate::foo::X }\n");
    write("src/a.rs", "pub const A: u8 = 1;\n");
    write("src/c.rs", "pub const C: u8 = 3;\n");
    let rewrite = |rel: &str, new_text: &str| RewrittenFile {
        path: root.join(rel),
        new_text: new_text.to_string(),
        edits: 1,
        old_line_count: std::fs::read_to_string(root.join(rel))
            .unwrap()
            .lines()
            .count() as u32,
    };
    let moved = |from: &str, to: &str| FileMove {
        from: root.join(from),
        to: root.join(to),
    };
    let outcome = RefactorOutcome {
        files: vec![
            rewrite("src/lib.rs", "mod bar;\nmod b;\nmod a;\n"),
            rewrite("src/foo.rs", "mod inner;\nmod extra;\npub use inner::f;\n"),
            rewrite("src/foo/inner.rs", "pub fn f() -> u8 { crate::bar::X }\n"),
            rewrite("src/a.rs", "pub const B: u8 = 1;\n"),
            rewrite("src/c.rs", "pub const A: u8 = 3;\n"),
        ],
        created: vec![RewrittenFile {
            path: root.join("src/foo/extra.rs"),
            new_text: "pub fn extra() {}\n".to_string(),
            edits: 1,
            old_line_count: 0,
        }],
        moves: vec![
            moved("src/foo.rs", "src/bar.rs"),
            moved("src/foo", "src/bar"),
            moved("src/a.rs", "src/b.rs"),
            moved("src/c.rs", "src/a.rs"),
        ],
    };
    let edit = super::workspace_edit_json(&outcome);
    let kinds: Vec<&str> = edit["documentChanges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|change| change["kind"].as_str().unwrap_or("edit"))
        .collect();
    assert_eq!(
        kinds,
        [
            "create", "edit", "edit", "edit", "edit", "edit", "edit", "rename", "rename", "rename",
            "rename"
        ]
    );
    prod_code_mcp::refactor::apply_workspace_edit(&root, &edit).unwrap();
    let read = |rel: &str| std::fs::read_to_string(root.join(rel)).ok();
    assert_eq!(
        read("src/lib.rs").as_deref(),
        Some("mod bar;\nmod b;\nmod a;\n")
    );
    assert_eq!(
        read("src/bar.rs").as_deref(),
        Some("mod inner;\nmod extra;\npub use inner::f;\n")
    );
    assert_eq!(
        read("src/bar/inner.rs").as_deref(),
        Some("pub fn f() -> u8 { crate::bar::X }\n")
    );
    assert_eq!(
        read("src/bar/extra.rs").as_deref(),
        Some("pub fn extra() {}\n")
    );
    assert_eq!(read("src/b.rs").as_deref(), Some("pub const B: u8 = 1;\n"));
    assert_eq!(read("src/a.rs").as_deref(), Some("pub const A: u8 = 3;\n"));
    for gone in ["src/foo.rs", "src/foo", "src/c.rs"] {
        assert!(!root.join(gone).exists(), "{gone} was moved away");
    }
    prod_code_mcp::sync::clear_sync_cache(&root);
}

/// The same through rust-analyzer: renaming the module `foo`, kept in `foo.rs` with its
/// submodule in `foo/`, moves both and rewrites every use, the one inside `foo/` included,
/// and the client lands all of it.
#[test]
fn a_module_rename_by_the_analyzer_lands_whole_in_the_checkout() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let write = |rel: &str, text: &str| {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(
        "src/lib.rs",
        "pub mod foo;\npub fn g() -> u8 { foo::inner::f() }\n",
    );
    write("src/foo.rs", "pub mod inner;\npub const X: u8 = 1;\n");
    write("src/foo/inner.rs", "pub fn f() -> u8 { crate::foo::X }\n");
    let engine = prod_code_engine_rust::RustEngine::load(&root).unwrap();
    // `foo` in `pub mod foo;` is line 1, column 9.
    let outcome = engine
        .rename(&root.join("src/lib.rs"), 1, 9, "bar")
        .expect("rename query")
        .expect("rename accepted");
    assert_eq!(outcome.moves.len(), 2, "{outcome:?}");
    let edit = super::workspace_edit_json(&outcome);
    prod_code_mcp::refactor::apply_workspace_edit(&root, &edit).unwrap();
    let read = |rel: &str| std::fs::read_to_string(root.join(rel)).ok();
    assert_eq!(
        read("src/lib.rs").as_deref(),
        Some("pub mod bar;\npub fn g() -> u8 { bar::inner::f() }\n")
    );
    assert_eq!(
        read("src/bar.rs").as_deref(),
        Some("pub mod inner;\npub const X: u8 = 1;\n")
    );
    assert_eq!(
        read("src/bar/inner.rs").as_deref(),
        Some("pub fn f() -> u8 { crate::bar::X }\n")
    );
    assert!(!root.join("src/foo.rs").exists() && !root.join("src/foo").exists());
    prod_code_mcp::sync::clear_sync_cache(&root);
}

#[cfg(test)]
mod analyzer_panic_tests {
    use super::*;

    #[test]
    fn a_panic_is_reported_as_one_unchecked_file_not_as_a_failed_request() {
        let payload: Box<dyn std::any::Any + Send> = Box::new("escaping bound vars.".to_string());
        let message = panic_message(payload);
        assert_eq!(message, "escaping bound vars.");
        assert_eq!(panic_message(Box::new("static text")), "static text");
        assert_eq!(panic_message(Box::new(42u8)), "no message");

        let report = analyzer_panic_report(&message);
        let items = report["items"].as_array().expect("items");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["severity"], 1);
        assert_eq!(items[0]["code"], ANALYZER_PANIC);
        assert_eq!(items[0]["range"]["start"]["line"], 0);
        let text = items[0]["message"].as_str().unwrap();
        assert!(text.contains("nothing in it was checked"), "{text}");
        assert!(
            text.contains("escaping bound vars. The compiler"),
            "one full stop: {text}"
        );
        assert!(text.contains("verify"), "{text}");
    }
}

#[cfg(test)]
mod exec_resilience_tests {
    use super::*;

    #[tokio::test]
    async fn refresh_engines_safely_handles_unloaded_or_failing_updates() {
        let storage = tempfile::tempdir().unwrap();
        let manager = WorkspaceManager::new();
        let ws_dir = storage.path().join("ws");
        std::fs::create_dir_all(&ws_dir).unwrap();
        let files = vec![FileDelta {
            relative_path: "src/lib.rs".to_string(),
            content: Some(b"pub fn dummy() {}\n".to_vec()),
            is_executable: false,
        }];
        refresh_engines(&manager, &ws_dir, &files).await;
    }

    #[test]
    fn test_effective_prune_timeouts() {
        let cli = ServerCli {
            bind: "0.0.0.0:9400".parse().unwrap(),
            socket_path: None,
            storage: PathBuf::from("/tmp/storage"),
            idle_evict_secs: 1800,
            engine_reserve_mib: 0,
            max_concurrent_engine_loads: 0,
            prune_worktree_secs: 3600,
            prune_worktree_days: None,
            prune_workspace_secs: 86400,
            prune_workspace_days: None,
            prune_below_free_percent: 15,
            engines: vec![],
            shadow_dir: None,
            peers: String::new(),
            advertise: None,
            build_cache_ram: false,
            build_cache_dir: None,
        };
        // Defaults: 1 hour (3600s) for worktrees, 24 hours (86400s) for main workspaces
        assert_eq!(cli.effective_prune_worktree_secs(), 3600);
        assert_eq!(cli.effective_prune_workspace_secs(), 86400);

        // Days overrides
        let mut cli_days = cli.clone();
        cli_days.prune_worktree_days = Some(7);
        cli_days.prune_workspace_days = Some(3);
        assert_eq!(cli_days.effective_prune_worktree_secs(), 7 * 86_400);
        assert_eq!(cli_days.effective_prune_workspace_secs(), 3 * 86_400);

        // Disabling with 0
        let mut cli_disabled = cli.clone();
        cli_disabled.prune_worktree_secs = 0;
        cli_disabled.prune_workspace_days = Some(0);
        assert_eq!(cli_disabled.effective_prune_worktree_secs(), 0);
        assert_eq!(cli_disabled.effective_prune_workspace_secs(), 0);
    }

    #[test]
    fn test_prewarm_virtualenv_pycache_safe_on_missing_or_invalid() {
        let temp = tempfile::tempdir().unwrap();
        let empty_venv = temp.path().join("empty_venv");
        std::fs::create_dir_all(&empty_venv).unwrap();
        // Missing python binary -> Ok(0)
        let res = prewarm_virtualenv_pycache(&empty_venv);
        assert_eq!(res.unwrap(), 0);

        // Invalid non-executable python file -> Ok(0) without crashing
        let bin_dir = empty_venv.join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let py_file = bin_dir.join("python");
        std::fs::write(&py_file, b"not executable").unwrap();
        let res = prewarm_virtualenv_pycache(&empty_venv);
        assert_eq!(res.unwrap(), 0);
    }

    #[test]
    fn test_polyglot_compiler_cache_env_composition() {
        let temp = tempfile::tempdir().unwrap();
        let ws = temp.path().join("my-project");
        std::fs::create_dir_all(&ws).unwrap();

        let ram_target = temp.path().join("ram-target");
        let envs = polyglot_compiler_cache_env(&ws, false, Some(&ram_target));
        assert!(
            envs.iter()
                .any(|(k, v)| k == "CARGO_TARGET_DIR" && v == ram_target.to_str().unwrap())
        );
        assert!(
            envs.iter()
                .any(|(k, _)| k == "SWIFTPM_MODULECACHE_OVERRIDE")
        );
        assert!(envs.iter().any(|(k, _)| k == "SWIFT_MODULE_CACHE_PATH"));
        assert!(envs.iter().any(|(k, _)| k == "CLANG_MODULE_CACHE_PATH"));

        let ccache_envs = polyglot_compiler_cache_env(&ws, true, None);
        assert!(
            ccache_envs
                .iter()
                .any(|(k, v)| k == "CCACHE_BASEDIR" && v == ws.to_str().unwrap())
        );
        assert!(
            ccache_envs
                .iter()
                .any(|(k, v)| k == "CCACHE_NOHASHDIR" && v == "1")
        );
    }

    #[test]
    fn test_resolve_ram_build_cache_workspace_isolation() {
        let temp = tempfile::tempdir().unwrap();
        let ws1 = temp.path().join("repo-a");
        let ws2 = temp.path().join("repo-b");
        std::fs::create_dir_all(&ws1).unwrap();
        std::fs::create_dir_all(&ws2).unwrap();

        let custom_ram = temp.path().join("custom-shm");
        // Disabled returns None
        assert!(resolve_ram_build_cache(&ws1, false, Some(&custom_ram)).is_none());

        // Enabled creates isolated target directories
        let target1 = resolve_ram_build_cache(&ws1, true, Some(&custom_ram)).expect("target1");
        let target2 = resolve_ram_build_cache(&ws2, true, Some(&custom_ram)).expect("target2");

        assert!(target1.exists());
        assert!(target2.exists());
        assert_ne!(
            target1, target2,
            "workspaces must receive isolated RAM target directories"
        );
        assert!(target1.ends_with("target"));
        assert!(target2.ends_with("target"));
        assert!(target1.parent().unwrap().join(".last_used").exists());
        assert!(target2.parent().unwrap().join(".last_used").exists());
    }

    #[test]
    fn test_sweep_ram_build_caches_removes_old_dirs() {
        let temp = tempfile::tempdir().unwrap();
        let ram_base = temp.path().join("shm");
        let ws_dir = ram_base.join("ws-old");
        let target_dir = ws_dir.join("target");
        std::fs::create_dir_all(&target_dir).unwrap();

        // Fresh dir is not swept (not older than 24h)
        let swept = sweep_ram_build_caches(&ram_base);
        assert_eq!(swept, 0);
        assert!(ws_dir.exists());

        // Active lease prevents sweeping and is recognized as active
        let lease = RamBuildLease::acquire(&target_dir);
        assert!(lease.marker.is_some());
        let marker_path = lease.marker.clone().unwrap();
        assert!(marker_path.exists());
        assert!(is_ram_lease_active(&marker_path));
        let swept_active = sweep_ram_build_caches(&ram_base);
        assert_eq!(swept_active, 0);
        assert!(ws_dir.exists());
        assert!(marker_path.exists());

        // Dropping lease unlinks its marker file
        drop(lease);
        assert!(!marker_path.exists());

        // Stale lease marker (left by crashed process) is detected as inactive and cleaned up
        let stale_lease = ws_dir.join(".active_99999_1");
        std::fs::write(&stale_lease, b"stale").unwrap();
        assert!(stale_lease.exists());
        assert!(!is_ram_lease_active(&stale_lease));
        assert!(
            !stale_lease.exists(),
            "stale lease marker must be removed when unowned"
        );

        // Old dir (>24h) with stale lease marker is swept cleanly without being permanently protected
        let stale_old = ws_dir.join(".active_88888_2");
        std::fs::write(&stale_old, b"stale-old").unwrap();
        let last_used = ws_dir.join(".last_used");
        let f = std::fs::File::create(&last_used).unwrap();
        let past = std::time::SystemTime::now() - std::time::Duration::from_secs(100_000);
        f.set_modified(past).unwrap();
        drop(f);

        let swept_stale_old = sweep_ram_build_caches(&ram_base);
        assert_eq!(swept_stale_old, 1);
        assert!(
            !ws_dir.exists(),
            "stale crash-left marker must not permanently protect cache directory"
        );

        // Non-existent base dir returns 0 safely
        let swept_none = sweep_ram_build_caches(&temp.path().join("does_not_exist"));
        assert_eq!(swept_none, 0);
    }

    #[test]
    fn test_prewarm_virtualenv_pycache_does_not_execute_workspace_binary() {
        let temp = tempfile::tempdir().unwrap();
        let malicious_venv = temp.path().join("malicious_venv");
        let bin_dir = malicious_venv.join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let canary = temp.path().join("canary_executed.txt");
        let fake_python = bin_dir.join("python");
        // Script that would create the canary file if executed
        std::fs::write(
            &fake_python,
            format!("#!/bin/sh\ntouch {}\n", canary.display()),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake_python, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let _ = prewarm_virtualenv_pycache(&malicious_venv);
        assert!(
            !canary.exists(),
            "prewarm_virtualenv_pycache must never execute untrusted workspace python binary"
        );
    }

    #[test]
    fn test_resolve_ram_build_cache_false_env_does_not_enable() {
        assert!(!is_ram_cache_enabled_with(false, Some("false")));
        assert!(!is_ram_cache_enabled_with(false, Some("0")));
        assert!(!is_ram_cache_enabled_with(false, Some("no")));
        assert!(!is_ram_cache_enabled_with(false, Some("off")));
        assert!(!is_ram_cache_enabled_with(false, None));

        assert!(is_ram_cache_enabled_with(false, Some("true")));
        assert!(is_ram_cache_enabled_with(false, Some("1")));
        assert!(is_ram_cache_enabled_with(false, Some("yes")));
        assert!(is_ram_cache_enabled_with(false, Some("on")));
        assert!(is_ram_cache_enabled_with(true, Some("false")));
        assert!(is_ram_cache_enabled_with(true, None));
    }
}
