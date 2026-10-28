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
use prod_code_protocol::{ShadowHypothesis, ShadowRunResponse};
use tokio::io::AsyncWriteExt;

mod requests;

fn request() -> ShadowRunRequest {
    ShadowRunRequest {
        client_workspace_root: "/client/shadow-fixture".into(),
        base_workspace_name: Some("runner-fixture".into()),
        hypotheses: vec![ShadowHypothesis {
            name: "proposal".into(),
            files: vec![],
        }],
        command: vec![
            "sh".into(),
            "-c".into(),
            "cat choice.txt; printf :$SAMPLE".into(),
        ],
        env: vec![("SAMPLE".into(), "value".into())],
        timeout_secs: 10,
        subdir: None,
        parallel: 1,
        tail_bytes: 4,
        in_memory: false,
        client_agent: Some("test-agent".into()),
        client_host: Some("test-host".into()),
    }
}

fn storage() -> (tempfile::TempDir, crate::ServerState, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("storage");
    let workspace = root.join("runner-fixture");
    std::fs::create_dir_all(workspace.join("nested")).unwrap();
    std::fs::write(workspace.join("choice.txt"), "root").unwrap();
    std::fs::write(workspace.join("nested/choice.txt"), "nested").unwrap();
    let mut state = crate::ServerState::new(root);
    state.shadow_root = temp.path().join("shadows");
    (temp, state, workspace)
}

fn connection() -> (
    Framed<AnyStream, ProdCodeCodec>,
    Framed<AnyStream, ProdCodeCodec>,
) {
    let (client, server) = tokio::net::UnixStream::pair().unwrap();
    (
        Framed::new(AnyStream::Unix(client), ProdCodeCodec::new()),
        Framed::new(AnyStream::Unix(server), ProdCodeCodec::new()),
    )
}

#[tokio::test]
async fn shadow_subdirectories_environment_and_tail_limits_have_exact_results() {
    let (_temp, state, workspace) = storage();
    for (subdir, expected_len) in [
        ("nested", 12),
        ("", 10),
        ("/nested", 10),
        ("nested/../nested", 10),
        ("absent", 10),
    ] {
        let mut req = request();
        req.subdir = Some(subdir.into());
        let (mut client, mut server) = connection();
        let run = run_shadow(&state, &mut server, req);
        let receive = async {
            match client.next().await.unwrap().unwrap() {
                WireMessage::ShadowRunResponse(response) => response,
                other => panic!("expected shadow response, got {other:?}"),
            }
        };
        let (result, response): (_, ShadowRunResponse) = tokio::join!(run, receive);
        result.unwrap();
        assert_eq!(response.error, None);
        assert_eq!(response.server_workspace_root, workspace.to_string_lossy());
        assert_eq!(response.results.len(), 1);
        let result = &response.results[0];
        assert_eq!(result.name, "proposal");
        assert_eq!(
            (result.exit_code, result.error.as_deref(), result.timed_out),
            (Some(0), None, false)
        );
        assert_eq!(result.output_len, expected_len);
        assert_eq!(result.output_tail.as_deref(), Some(b"alue".as_slice()));
    }
    assert_eq!(
        std::fs::read_to_string(workspace.join("choice.txt")).unwrap(),
        "root"
    );
}

#[tokio::test]
async fn transport_security_production_wiring_scrubs_shadow_children() {
    crate::test_cache_support::isolated!();
    let (_temp, state, _) = storage();
    let inherited_canaries = [
        ("PROD_CODE_AUTH_TOKEN", "CANARY_INHERITED_AUTH_SHADOW"),
        (
            "PROD_CODE_AUTH_TOKEN_FILE",
            "CANARY_INHERITED_AUTH_FILE_SHADOW",
        ),
        ("PROD_CODE_TLS_MODE", "CANARY_INHERITED_TLS_MODE_SHADOW"),
        ("PROD_CODE_TLS_CERT", "CANARY_INHERITED_TLS_CERT_SHADOW"),
        ("PROD_CODE_TLS_KEY", "CANARY_INHERITED_TLS_KEY_SHADOW"),
        ("PROD_CODE_TLS_CA", "CANARY_INHERITED_TLS_CA_SHADOW"),
        ("PROD_CODE_TLS_PIN", "CANARY_INHERITED_TLS_PIN_SHADOW"),
        (
            "PROD_CODE_TLS_SERVER_NAME",
            "CANARY_INHERITED_TLS_NAME_SHADOW",
        ),
    ];
    let request_canaries = [
        ("PROD_CODE_AUTH_TOKEN", "CANARY_REQUEST_AUTH_SHADOW"),
        (
            "PROD_CODE_AUTH_TOKEN_FILE",
            "CANARY_REQUEST_AUTH_FILE_SHADOW",
        ),
        ("PROD_CODE_TLS_MODE", "CANARY_REQUEST_TLS_MODE_SHADOW"),
        ("PROD_CODE_TLS_CERT", "CANARY_REQUEST_TLS_CERT_SHADOW"),
        ("PROD_CODE_TLS_KEY", "CANARY_REQUEST_TLS_KEY_SHADOW"),
        ("PROD_CODE_TLS_CA", "CANARY_REQUEST_TLS_CA_SHADOW"),
        ("PROD_CODE_TLS_PIN", "CANARY_REQUEST_TLS_PIN_SHADOW"),
        (
            "PROD_CODE_TLS_SERVER_NAME",
            "CANARY_REQUEST_TLS_NAME_SHADOW",
        ),
    ];
    unsafe {
        for (name, value) in inherited_canaries {
            std::env::set_var(name, value);
        }
    }
    for canaries in [&inherited_canaries[..], &request_canaries[..]] {
        let control = std::process::Command::new("env")
            .env_clear()
            .envs(canaries.iter().copied())
            .output()
            .unwrap();
        let control_stdout = String::from_utf8_lossy(&control.stdout);
        assert!(
            canaries
                .iter()
                .all(|(_, value)| control_stdout.contains(value))
        );
    }

    let mut req = request();
    req.command = vec!["env".into()];
    req.env = request_canaries
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
    req.tail_bytes = 16 * 1024;
    let (mut client, mut server) = connection();
    let run = run_shadow(&state, &mut server, req);
    let receive = async {
        match client.next().await.unwrap().unwrap() {
            WireMessage::ShadowRunResponse(response) => response,
            other => panic!("expected shadow response, got {other:?}"),
        }
    };
    let (result, response) = tokio::join!(run, receive);
    result.unwrap();
    assert_eq!(response.error, None);
    assert_eq!(response.results.len(), 1);
    assert_eq!(response.results[0].exit_code, Some(0));
    let stdout = String::from_utf8_lossy(
        response.results[0]
            .output_tail
            .as_deref()
            .unwrap_or_default(),
    );
    for (name, value) in inherited_canaries.iter().chain(request_canaries.iter()) {
        assert!(!stdout.contains(&format!("{name}=")), "leaked {name}");
        assert!(!stdout.contains(value), "leaked canary for {name}");
    }
    unsafe {
        for (name, _) in inherited_canaries {
            std::env::remove_var(name);
        }
    }
}

#[tokio::test]
async fn shadow_answers_pings_and_cancels_running_and_queued_jobs_when_the_client_leaves() {
    for leave in ["disconnect", "eof", "invalid-frame"] {
        let (temp, state, workspace) = storage();
        let ready = temp.path().join("ready");
        let mut req = request();
        req.command = vec![
            "sh".into(),
            "-c".into(),
            "printf ready > \"$READY_PATH\"; sleep 30".into(),
        ];
        req.env
            .push(("READY_PATH".into(), ready.to_string_lossy().into_owned()));
        req.hypotheses[0].files = vec![prod_code_protocol::FileDelta {
            relative_path: "choice.txt".into(),
            content: Some(b"proposed".to_vec()),
            is_executable: false,
        }];
        let mut queued = req.hypotheses[0].clone();
        queued.name = "queued".into();
        req.hypotheses.push(queued);
        let (mut client, mut server) = connection();
        let run = run_shadow(&state, &mut server, req);
        let interact = async {
            tokio::time::timeout(Duration::from_secs(10), async {
                while std::fs::read_to_string(&ready).ok().as_deref() != Some("ready") {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("the first command starts");
            assert_eq!(std::fs::read_to_string(&ready).unwrap(), "ready");
            client.send(WireMessage::Pong).await.unwrap(); // Unrelated messages do not end a run.
            client.send(WireMessage::Ping).await.unwrap();
            assert!(matches!(
                client.next().await.unwrap().unwrap(),
                WireMessage::Pong
            ));
            match leave {
                "disconnect" => client
                    .send(WireMessage::Disconnect {
                        reason: "finished".into(),
                    })
                    .await
                    .unwrap(),
                "invalid-frame" => client.get_mut().write_all(&[0xff; 4]).await.unwrap(),
                _ => {
                    client.get_mut().shutdown().await.unwrap();
                }
            }
            // Keep the read side open until run_shadow has observed the chosen termination.
        };
        let (result, ()) = tokio::time::timeout(Duration::from_secs(15), async {
            tokio::join!(run, interact)
        })
        .await
        .unwrap();
        if leave == "invalid-frame" {
            assert_eq!(
                result.unwrap_err().to_string(),
                "Frame size 4294967295 exceeds maximum 268435456"
            );
        } else {
            result.unwrap();
        }
        assert_eq!(
            std::fs::read_to_string(workspace.join("choice.txt")).unwrap(),
            "root"
        );
        assert_eq!(std::fs::read_to_string(&ready).unwrap(), "ready");
    }
}
