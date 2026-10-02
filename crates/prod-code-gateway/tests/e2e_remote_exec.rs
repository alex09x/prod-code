use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{
    ExecChunk, ProdCodeCodec, RemoteExecCommand, RemoteExecDiagnostic, RemoteExecFormat,
    RemoteExecLanguage, RemoteExecRequest, RemoteExecResult, RemoteExecStream, RemoteExecTestEvent,
    WireMessage,
};
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tokio_util::codec::Framed;

async fn start_test_gateway() -> (SocketAddr, tokio::task::JoinHandle<()>, tempfile::TempDir) {
    let temp_dir = tempfile::tempdir().unwrap();
    let storage_root = temp_dir.path().to_path_buf();

    // Create a dummy workspace folder
    let ws_dir = storage_root.join("my-test-workspace");
    std::fs::create_dir_all(&ws_dir).unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let storage_root_clone = storage_root.clone();
    let handle = tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let storage = storage_root_clone.clone();
            tokio::spawn(async move {
                let mut framed = Framed::new(socket, ProdCodeCodec::new());
                while let Some(Ok(msg)) = framed.next().await {
                    match msg {
                        WireMessage::RemoteExecRequest(req) => {
                            let workspace = storage.join(
                                req.base_workspace_name
                                    .as_deref()
                                    .unwrap_or("my-test-workspace"),
                            );
                            if !workspace.is_dir() {
                                let _ = framed
                                    .send(WireMessage::RemoteExecResult(RemoteExecResult {
                                        exit_code: None,
                                        duration_ms: 0,
                                        server_workspace_root: workspace.to_string_lossy().to_string(),
                                        timed_out: false,
                                        error: Some("workspace not synced".into()),
                                        usage: None,
                                        platform: None,
                                        diagnostics: Vec::new(),
                                        tests_passed: 0,
                                        tests_failed: 0,
                                        tests_skipped: 0,
                                        test_failures: Vec::new(),
                                        benches: Vec::new(),
                                    }))
                                    .await;
                                return;
                            }
                            if req.command == RemoteExecCommand::Check && req.language == RemoteExecLanguage::Rust {
                                // Simulate cargo compiler JSON message stream
                                let diag_json = r#"{"reason":"compiler-message","package_id":"foo","message":{"level":"error","code":{"code":"E0308"},"message":"mismatched types","spans":[{"file_name":"src/lib.rs","line_start":10,"column_start":5,"is_primary":true,"label":"expected u32, found &str"}],"rendered":"error[E0308]: mismatched types\n"}}"#;
                                let _ = framed
                                    .send(WireMessage::RemoteExecStream(RemoteExecStream::Chunk(
                                        ExecChunk {
                                            stderr: false,
                                            data: Some(format!("{diag_json}\n").into_bytes()),
                                        },
                                    )))
                                    .await;

                                if req.format == RemoteExecFormat::Json {
                                    if let Some(stream_ev) = prod_code_protocol::parse_cargo_json_event(diag_json) {
                                        let _ = framed.send(WireMessage::RemoteExecStream(stream_ev)).await;
                                    }
                                }

                                let _ = framed
                                    .send(WireMessage::RemoteExecResult(RemoteExecResult {
                                        exit_code: Some(1),
                                        duration_ms: 45,
                                        server_workspace_root: workspace.to_string_lossy().to_string(),
                                        timed_out: false,
                                        error: None,
                                        usage: None,
                                        platform: Some("linux x86_64".into()),
                                        diagnostics: vec![RemoteExecDiagnostic {
                                            level: "error".into(),
                                            code: Some("E0308".into()),
                                            message: "mismatched types".into(),
                                            spans: vec![],
                                            rendered: Some("error[E0308]: mismatched types".into()),
                                            suggestion: None,
                                        }],
                                        tests_passed: 0,
                                        tests_failed: 0,
                                        tests_skipped: 0,
                                        test_failures: Vec::new(),
                                        benches: Vec::new(),
                                    }))
                                    .await;
                            } else if req.command == RemoteExecCommand::Test && req.language == RemoteExecLanguage::Go {
                                // Simulate Go test JSON stream
                                let go_pass = r#"{"Time":"2026-10-02T12:00:00Z","Action":"pass","Package":"pkg/auth","Test":"TestLogin","Elapsed":0.012}"#;
                                let go_fail = r#"{"Time":"2026-10-02T12:00:01Z","Action":"fail","Package":"pkg/auth","Test":"TestRefresh","Elapsed":0.034}"#;

                                for line in [go_pass, go_fail] {
                                    let _ = framed
                                        .send(WireMessage::RemoteExecStream(RemoteExecStream::Chunk(
                                            ExecChunk {
                                                stderr: false,
                                                data: Some(format!("{line}\n").into_bytes()),
                                            },
                                        )))
                                        .await;
                                    if let Some(stream_ev) = prod_code_protocol::parse_go_test_json_event(line) {
                                        let _ = framed.send(WireMessage::RemoteExecStream(stream_ev)).await;
                                    }
                                }

                                let _ = framed
                                    .send(WireMessage::RemoteExecResult(RemoteExecResult {
                                        exit_code: Some(1),
                                        duration_ms: 120,
                                        server_workspace_root: workspace.to_string_lossy().to_string(),
                                        timed_out: false,
                                        error: None,
                                        usage: None,
                                        platform: Some("linux x86_64".into()),
                                        diagnostics: Vec::new(),
                                        tests_passed: 1,
                                        tests_failed: 1,
                                        tests_skipped: 0,
                                        test_failures: vec![RemoteExecTestEvent::Failed {
                                            name: "pkg/auth.TestRefresh".into(),
                                            duration_ms: Some(34),
                                            message: None,
                                            assertion_diff: None,
                                            backtrace: None,
                                            output: None,
                                        }],
                                        benches: Vec::new(),
                                    }))
                                    .await;
                            } else {
                                let _ = framed
                                    .send(WireMessage::RemoteExecResult(RemoteExecResult {
                                        exit_code: Some(0),
                                        duration_ms: 10,
                                        server_workspace_root: workspace.to_string_lossy().to_string(),
                                        timed_out: false,
                                        error: None,
                                        usage: None,
                                        platform: None,
                                        diagnostics: Vec::new(),
                                        tests_passed: 0,
                                        tests_failed: 0,
                                        tests_skipped: 0,
                                        test_failures: Vec::new(),
                                        benches: Vec::new(),
                                    }))
                                    .await;
                            }
                        }
                        WireMessage::Disconnect { .. } => return,
                        _ => {}
                    }
                }
            });
        }
    });

    (addr, handle, temp_dir)
}

#[tokio::test]
async fn test_polyglot_remote_exec_rust_check_flow() {
    let (addr, _server_handle, _temp) = start_test_gateway().await;
    let stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let mut framed = Framed::new(stream, ProdCodeCodec::new());

    let req = RemoteExecRequest {
        client_workspace_root: "/Users/test/my-test-workspace".into(),
        base_workspace_name: Some("my-test-workspace".into()),
        language: RemoteExecLanguage::Rust,
        command: RemoteExecCommand::Check,
        args: vec!["--lib".into()],
        env: vec![],
        format: RemoteExecFormat::Json,
        timeout_secs: 30,
        pull_changes: false,
        subdir: None,
        client_agent: Some("agent-unit-test".into()),
        client_host: Some("localhost".into()),
    };

    framed.send(WireMessage::RemoteExecRequest(req)).await.unwrap();

    let mut stream_events = Vec::new();
    let mut final_result = None;

    while let Some(Ok(msg)) = framed.next().await {
        match msg {
            WireMessage::RemoteExecStream(ev) => {
                stream_events.push(ev);
            }
            WireMessage::RemoteExecResult(res) => {
                final_result = Some(res);
                break;
            }
            other => panic!("unexpected wire message: {other:?}"),
        }
    }

    assert!(final_result.is_some());
    let res = final_result.unwrap();
    assert_eq!(res.exit_code, Some(1));
    assert_eq!(res.diagnostics.len(), 1);
    assert_eq!(res.diagnostics[0].code.as_deref(), Some("E0308"));

    // Verify stream contained both raw chunk and structured diagnostic
    let has_chunk = stream_events.iter().any(|ev| matches!(ev, RemoteExecStream::Chunk(_)));
    let has_diag = stream_events.iter().any(|ev| matches!(ev, RemoteExecStream::Diagnostic(d) if d.code.as_deref() == Some("E0308")));
    assert!(has_chunk);
    assert!(has_diag);
}

#[tokio::test]
async fn test_polyglot_remote_exec_go_test_flow() {
    let (addr, _server_handle, _temp) = start_test_gateway().await;
    let stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let mut framed = Framed::new(stream, ProdCodeCodec::new());

    let req = RemoteExecRequest {
        client_workspace_root: "/Users/test/my-test-workspace".into(),
        base_workspace_name: Some("my-test-workspace".into()),
        language: RemoteExecLanguage::Go,
        command: RemoteExecCommand::Test,
        args: vec!["-run".into(), "Test".into()],
        env: vec![],
        format: RemoteExecFormat::Json,
        timeout_secs: 30,
        pull_changes: false,
        subdir: None,
        client_agent: Some("agent-unit-test".into()),
        client_host: Some("localhost".into()),
    };

    framed.send(WireMessage::RemoteExecRequest(req)).await.unwrap();

    let mut stream_events = Vec::new();
    let mut final_result = None;

    while let Some(Ok(msg)) = framed.next().await {
        match msg {
            WireMessage::RemoteExecStream(ev) => {
                stream_events.push(ev);
            }
            WireMessage::RemoteExecResult(res) => {
                final_result = Some(res);
                break;
            }
            other => panic!("unexpected wire message: {other:?}"),
        }
    }

    assert!(final_result.is_some());
    let res = final_result.unwrap();
    assert_eq!(res.exit_code, Some(1));
    assert_eq!(res.tests_passed, 1);
    assert_eq!(res.tests_failed, 1);
    assert_eq!(res.test_failures.len(), 1);
    assert_eq!(res.test_failures[0].name(), "pkg/auth.TestRefresh");

    let has_pass_event = stream_events.iter().any(|ev| matches!(ev, RemoteExecStream::TestEvent(RemoteExecTestEvent::Passed { name, .. }) if name == "pkg/auth.TestLogin"));
    let has_fail_event = stream_events.iter().any(|ev| matches!(ev, RemoteExecStream::TestEvent(RemoteExecTestEvent::Failed { name, .. }) if name == "pkg/auth.TestRefresh"));
    assert!(has_pass_event);
    assert!(has_fail_event);
}

#[tokio::test]
async fn test_polyglot_remote_exec_unsynced_workspace_error() {
    let (addr, _server_handle, _temp) = start_test_gateway().await;
    let stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let mut framed = Framed::new(stream, ProdCodeCodec::new());

    let req = RemoteExecRequest {
        client_workspace_root: "/Users/test/nonexistent-workspace".into(),
        base_workspace_name: Some("nonexistent-workspace".into()),
        language: RemoteExecLanguage::Rust,
        command: RemoteExecCommand::Check,
        args: vec![],
        env: vec![],
        format: RemoteExecFormat::Raw,
        timeout_secs: 10,
        pull_changes: false,
        subdir: None,
        client_agent: None,
        client_host: None,
    };

    framed.send(WireMessage::RemoteExecRequest(req)).await.unwrap();

    let msg = framed.next().await.unwrap().unwrap();
    match msg {
        WireMessage::RemoteExecResult(res) => {
            assert!(res.error.is_some());
            assert!(res.error.unwrap().contains("not synced"));
            assert_eq!(res.exit_code, None);
        }
        other => panic!("expected RemoteExecResult, got {other:?}"),
    }
}
