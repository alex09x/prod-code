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
use prod_code_protocol::ExecChanges;

struct Run {
    storage: tempfile::TempDir,
    messages: Vec<WireMessage>,
}

impl Run {
    fn workspace(&self) -> PathBuf {
        self.storage.path().join("ws")
    }

    fn exit(&self) -> &ExecExit {
        match self.messages.last() {
            Some(WireMessage::ExecExit(e)) => e,
            other => panic!("no exit at the end: {other:?}"),
        }
    }

    fn output(&self, stderr: bool) -> String {
        let bytes: Vec<u8> = self
            .messages
            .iter()
            .filter_map(|m| match m {
                WireMessage::ExecChunk(c) if c.stderr == stderr => c.data.clone(),
                _ => None,
            })
            .flatten()
            .collect();
        String::from_utf8(bytes).unwrap()
    }
}

fn request(command: &[&str]) -> ExecRequest {
    ExecRequest {
        client_workspace_root: "/client/ws".to_string(),
        base_workspace_name: Some("ws".to_string()),
        command: command.iter().map(ToString::to_string).collect(),
        env: Vec::new(),
        timeout_secs: 20,
        pull_changes: false,
        subdir: None,
        client_agent: None,
        client_host: Some("unit-host".to_string()),
    }
}

fn sh(script: &str) -> ExecRequest {
    request(&["sh", "-c", script])
}

async fn run(req: ExecRequest, files: &[(&str, &str)], make_workspace: bool) -> Run {
    let storage = tempfile::tempdir().unwrap();
    if make_workspace {
        std::fs::create_dir_all(storage.path().join("ws")).unwrap();
        for (rel, body) in files {
            let path = storage.path().join("ws").join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = tokio::net::TcpStream::connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    let (server, _) = listener.accept().await.unwrap();
    let root = storage.path().to_path_buf();
    let metrics_dir = tempfile::tempdir().unwrap();
    let gateway = tokio::spawn(async move {
        let metrics = metrics::Metrics::new(metrics_dir.path().to_path_buf());
        let manager = WorkspaceManager::new();
        let mut framed = Framed::new(AnyStream::from(server), ProdCodeCodec::new());
        run_exec(&root, &metrics, &manager, &mut framed, req).await
    });
    let mut client = Framed::new(client, ProdCodeCodec::new());
    let mut messages = Vec::new();
    while let Some(Ok(msg)) = client.next().await {
        let done = matches!(msg, WireMessage::ExecExit(_));
        messages.push(msg);
        if done {
            break;
        }
    }
    gateway.await.unwrap().unwrap();
    Run { storage, messages }
}

#[tokio::test]
async fn a_request_that_cannot_run_is_refused_with_its_exact_reason() {
    let missing_dir = |sub: &str| {
        let mut req = sh("true");
        req.subdir = Some(sub.to_string());
        req
    };
    let mut huge = sh("true");
    huge.timeout_secs = MAX_REMOTE_EXEC_TIMEOUT_SECS + 1;
    for (name, req, make_workspace, expected) in [
        (
            "no copy",
            sh("true"),
            false,
            "is not synced to this gateway",
        ),
        ("empty", request(&[]), true, "empty command"),
        (
            "timeout",
            huge,
            true,
            "timeout_secs (604801) exceeds maximum allowed (604800)",
        ),
        (
            "parent",
            missing_dir("../outside"),
            true,
            "invalid working directory '../outside'",
        ),
        (
            "absolute",
            missing_dir("/etc"),
            true,
            "invalid working directory '/etc'",
        ),
        (
            "missing",
            missing_dir("nope/deeper"),
            true,
            "working directory 'nope/deeper' does not exist in workspace ",
        ),
        (
            "spawn",
            request(&["no-such-program-for-unit-tests"]),
            true,
            "failed to start no-such-program-for-unit-tests: ",
        ),
    ] {
        let run = run(req, &[], make_workspace).await;
        let exit = run.exit();
        let error = exit.error.as_deref().unwrap_or_default();
        assert!(error.contains(expected), "{name}: {error}");
        assert_eq!(exit.exit_code, None, "{name}");
        assert_eq!(exit.duration_ms, 0, "{name}");
        assert_eq!(run.messages.len(), 1, "{name}: nothing else is sent");
    }
}

#[tokio::test]
async fn a_command_runs_in_its_subdir_with_its_env_and_exit_status() {
    let mut req = sh("pwd; echo \"$UNIT_VAR\"; echo oops >&2; exit 3");
    req.subdir = Some("pkg".to_string());
    req.env = vec![("UNIT_VAR".to_string(), "value".to_string())];
    let run = run(req, &[("pkg/a.txt", "a")], true).await;
    let exit = run.exit();
    assert_eq!(exit.exit_code, Some(3));
    assert_eq!(exit.error, None);
    assert!(!exit.timed_out);
    assert!(exit.usage.is_some());
    assert_eq!(
        exit.server_workspace_root,
        run.workspace().to_string_lossy()
    );
    let out = run.output(false);
    let (dir, var) = out.trim_end().split_once('\n').unwrap();
    assert!(dir.ends_with("/ws/pkg"), "{dir}");
    assert_eq!(var, "value");
    assert_eq!(run.output(true), "oops\n");
}

#[tokio::test]
async fn transport_security_production_wiring_scrubs_exec_children() {
    let canaries = [
        ("PROD_CODE_AUTH_TOKEN", "CANARY_AUTH_TOKEN_EXEC"),
        ("PROD_CODE_AUTH_TOKEN_FILE", "CANARY_AUTH_FILE_EXEC"),
        ("PROD_CODE_TLS_MODE", "CANARY_TLS_MODE_EXEC"),
        ("PROD_CODE_TLS_CERT", "CANARY_TLS_CERT_EXEC"),
        ("PROD_CODE_TLS_KEY", "CANARY_TLS_KEY_EXEC"),
        ("PROD_CODE_TLS_CA", "CANARY_TLS_CA_EXEC"),
        ("PROD_CODE_TLS_PIN", "CANARY_TLS_PIN_EXEC"),
        ("PROD_CODE_TLS_SERVER_NAME", "CANARY_TLS_NAME_EXEC"),
    ];
    let control = std::process::Command::new("env")
        .env_clear()
        .envs(canaries)
        .output()
        .unwrap();
    let control_stdout = String::from_utf8_lossy(&control.stdout);
    assert!(
        canaries
            .iter()
            .all(|(_, value)| control_stdout.contains(value))
    );

    let mut req = sh("env");
    req.env = canaries
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
    let run = run(req, &[], true).await;
    assert_eq!(run.exit().exit_code, Some(0));
    let stdout = run.output(false);
    for (name, value) in canaries {
        assert!(!stdout.contains(&format!("{name}=")), "leaked {name}");
        assert!(!stdout.contains(value), "leaked canary for {name}");
    }
}

#[tokio::test]
async fn files_a_command_changed_are_sent_back_before_the_exit() {
    let mut req = sh("printf new > made.txt; printf changed > old.txt; rm gone.txt");
    req.pull_changes = true;
    let run = run(
        req,
        &[("old.txt", "old"), ("gone.txt", "g"), ("same.txt", "s")],
        true,
    )
    .await;
    assert_eq!(run.exit().exit_code, Some(0));
    let changes: Vec<&ExecChanges> = run
        .messages
        .iter()
        .filter_map(|m| match m {
            WireMessage::ExecChanges(c) => Some(c),
            _ => None,
        })
        .collect();
    assert_eq!(changes.len(), 1);
    let summary: Vec<(&str, Option<&[u8]>)> = changes[0]
        .files
        .iter()
        .map(|f| (f.relative_path.as_str(), f.content.as_deref()))
        .collect();
    assert_eq!(
        summary,
        [
            ("gone.txt", None),
            ("made.txt", Some(&b"new"[..])),
            ("old.txt", Some(&b"changed"[..])),
        ]
    );
    assert!(matches!(
        run.messages[run.messages.len() - 2],
        WireMessage::ExecChanges(_)
    ));
}

#[tokio::test]
async fn a_command_past_its_deadline_is_killed_and_flagged() {
    let mut req = request(&["sleep", "30"]);
    req.timeout_secs = 1;
    let started = std::time::Instant::now();
    let run = run(req, &[], true).await;
    assert!(started.elapsed() < Duration::from_secs(15));
    let exit = run.exit();
    assert!(exit.timed_out);
    assert_eq!(exit.exit_code, None);
    assert_eq!(exit.error, None);
}
