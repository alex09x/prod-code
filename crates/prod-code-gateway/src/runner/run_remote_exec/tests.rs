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

    fn result(&self) -> &RemoteExecResult {
        match self.messages.last() {
            Some(WireMessage::RemoteExecResult(r)) => r,
            other => panic!("no result at the end: {other:?}"),
        }
    }

    fn output(&self, stderr: bool) -> String {
        let bytes: Vec<u8> = self
            .messages
            .iter()
            .filter_map(|m| match m {
                WireMessage::RemoteExecStream(RemoteExecStream::Chunk(c)) if c.stderr == stderr => {
                    c.data.clone()
                }
                _ => None,
            })
            .flatten()
            .collect();
        String::from_utf8(bytes).unwrap()
    }
}

fn request(command: &str, args: &[&str]) -> RemoteExecRequest {
    RemoteExecRequest {
        client_workspace_root: "/client/ws".to_string(),
        base_workspace_name: Some("ws".to_string()),
        language: RemoteExecLanguage::Generic,
        command: RemoteExecCommand::Custom(command.to_string()),
        args: args.iter().map(ToString::to_string).collect(),
        env: Vec::new(),
        format: RemoteExecFormat::Raw,
        timeout_secs: 20,
        pull_changes: false,
        subdir: None,
        client_agent: Some("unit".to_string()),
        client_host: None,
    }
}

fn sh(script: &str) -> RemoteExecRequest {
    request("sh", &["-c", script])
}

/// Runs the request in a workspace `ws` holding `files` and collects everything sent.
async fn run(req: RemoteExecRequest, files: &[(&str, &str)], make_workspace: bool) -> Run {
    let storage = tempfile::tempdir().unwrap();
    if make_workspace {
        for (rel, body) in files {
            let path = storage.path().join("ws").join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }
        std::fs::create_dir_all(storage.path().join("ws")).unwrap();
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
        run_remote_exec(&root, &metrics, &manager, &mut framed, req).await
    });
    let mut client = Framed::new(client, ProdCodeCodec::new());
    let mut messages = Vec::new();
    while let Some(Ok(msg)) = client.next().await {
        let done = matches!(msg, WireMessage::RemoteExecResult(_));
        messages.push(msg);
        if done {
            break;
        }
    }
    gateway.await.unwrap().unwrap();
    Run { storage, messages }
}

#[tokio::test]
async fn a_request_that_cannot_run_is_refused_with_its_reason() {
    let mut huge = sh("true");
    huge.timeout_secs = MAX_REMOTE_EXEC_TIMEOUT_SECS + 1;
    let mut bad_subdir = sh("true");
    bad_subdir.subdir = Some("../outside".to_string());
    for (name, req, make_workspace, expected) in [
        (
            "no copy",
            sh("true"),
            false,
            "is not synced to this gateway",
        ),
        ("empty", request("", &[]), true, "empty command"),
        (
            "timeout",
            huge,
            true,
            "timeout_secs (604801) exceeds maximum allowed (604800)",
        ),
        (
            "spawn",
            request("no-such-program-for-unit-tests", &[]),
            true,
            "failed to start no-such-program-for-unit-tests: ",
        ),
    ] {
        let run = run(req, &[], make_workspace).await;
        let result = run.result();
        let error = result.error.as_deref().unwrap_or_default();
        assert!(error.contains(expected), "{name}: {error}");
        assert_eq!(result.exit_code, None, "{name}");
        assert!(!result.timed_out, "{name}");
        assert_eq!(result.duration_ms, 0, "{name}");
        assert_eq!(run.messages.len(), 1, "{name}: nothing else is sent");
    }
    // An unusable subdirectory falls back to the workspace root rather than escaping it.
    let run = run(
        {
            let mut req = sh("pwd");
            req.subdir = bad_subdir.subdir;
            req
        },
        &[],
        true,
    )
    .await;
    assert_eq!(run.result().exit_code, Some(0));
    assert_eq!(
        std::fs::canonicalize(run.output(false).trim()).unwrap(),
        std::fs::canonicalize(run.workspace()).unwrap()
    );
}

#[tokio::test]
async fn a_command_runs_in_its_subdir_with_its_env_and_exit_status() {
    let mut req = sh("pwd; echo \"$UNIT_VAR\"; echo oops >&2; exit 3");
    req.subdir = Some("pkg".to_string());
    req.env = vec![("UNIT_VAR".to_string(), "value".to_string())];
    let run = run(req, &[("pkg/a.txt", "a")], true).await;
    let result = run.result();
    assert_eq!(result.exit_code, Some(3));
    assert_eq!(result.error, None);
    assert!(!result.timed_out);
    assert!(result.usage.is_some());
    assert_eq!(result.platform, Some(prod_code_protocol::platform()));
    assert_eq!(
        result.server_workspace_root,
        run.workspace().to_string_lossy()
    );
    let out = run.output(false);
    let (dir, var) = out.trim_end().split_once('\n').unwrap();
    assert!(dir.ends_with("/ws/pkg"), "{dir}");
    assert_eq!(var, "value");
    assert_eq!(run.output(true), "oops\n");
    assert!(
        !run.messages
            .iter()
            .any(|m| matches!(m, WireMessage::ExecChanges(_))),
        "without pull_changes nothing is sent back"
    );
}

#[tokio::test]
async fn files_a_command_changed_are_sent_back_before_the_result() {
    let mut req = sh("printf new > made.txt; printf changed > old.txt; rm gone.txt");
    req.pull_changes = true;
    let run = run(
        req,
        &[("old.txt", "old"), ("gone.txt", "g"), ("same.txt", "s")],
        true,
    )
    .await;
    assert_eq!(run.result().exit_code, Some(0));
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
    // The changes arrive before the final result.
    let at = |pred: fn(&WireMessage) -> bool| run.messages.iter().position(pred).unwrap();
    assert!(
        at(|m| matches!(m, WireMessage::ExecChanges(_)))
            < at(|m| matches!(m, WireMessage::RemoteExecResult(_)))
    );
}

#[tokio::test]
async fn json_output_is_parsed_into_events_counts_and_failures() {
    let mut req = request(
        "sh",
        &[
            "-c",
            "printf '%s\\n' '{\"type\":\"test\",\"event\":\"ok\",\"name\":\"t1\"}' 'test t2 ... FAILED' \
             '{\"reason\":\"compiler-message\",\"message\":{\"level\":\"warning\",\"message\":\"careful\"}}'; \
             printf 'test t3 ... ignored'; exit 101",
        ],
    );
    req.language = RemoteExecLanguage::Rust;
    req.format = RemoteExecFormat::Json;
    let run = run(req, &[], true).await;
    let result = run.result();
    assert_eq!(result.exit_code, Some(101));
    assert_eq!(
        (
            result.tests_passed,
            result.tests_failed,
            result.tests_skipped
        ),
        (1, 1, 1)
    );
    assert_eq!(result.test_failures.len(), 1);
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(result.diagnostics[0].message, "careful");
    let events = run
        .messages
        .iter()
        .filter(|m| {
            matches!(
                m,
                WireMessage::RemoteExecStream(RemoteExecStream::TestEvent(_))
            )
        })
        .count();
    assert_eq!(events, 3);
}

#[tokio::test]
async fn a_command_past_its_deadline_is_killed_and_flagged() {
    let mut req = request("sleep", &["30"]);
    req.timeout_secs = 1;
    let started = std::time::Instant::now();
    let run = run(req, &[], true).await;
    assert!(started.elapsed() < Duration::from_secs(15));
    let result = run.result();
    assert!(result.timed_out);
    assert_eq!(result.exit_code, None);
    assert_eq!(result.error, None);
}

#[tokio::test]
async fn a_client_that_leaves_ends_the_command_and_its_changes_are_undone() {
    for pull_changes in [false, true] {
        let storage = tempfile::tempdir().unwrap();
        let workspace = storage.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("keep.txt"), "kept").unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = tokio::net::TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (server, _) = listener.accept().await.unwrap();
        let root = storage.path().to_path_buf();
        let metrics_dir = tempfile::tempdir().unwrap();
        let mut req = sh("printf scratch > made.txt; echo ready; sleep 60");
        req.pull_changes = pull_changes;
        req.timeout_secs = 120;
        let gateway = tokio::spawn(async move {
            let metrics = metrics::Metrics::new(metrics_dir.path().to_path_buf());
            let manager = WorkspaceManager::new();
            let mut framed = Framed::new(AnyStream::from(server), ProdCodeCodec::new());
            run_remote_exec(&root, &metrics, &manager, &mut framed, req).await
        });
        let mut client = Framed::new(client, ProdCodeCodec::new());
        let mut seen = String::new();
        while !seen.contains("ready") {
            match tokio::time::timeout(Duration::from_secs(20), client.next()).await {
                Ok(Some(Ok(WireMessage::RemoteExecStream(RemoteExecStream::Chunk(c))))) => {
                    seen.push_str(&String::from_utf8_lossy(&c.data.unwrap_or_default()));
                }
                other => panic!("no output from the command: {other:?}"),
            }
        }
        drop(client);
        tokio::time::timeout(Duration::from_secs(20), gateway)
            .await
            .expect("the command is killed, not left to run out its sleep")
            .unwrap()
            .unwrap();
        assert_eq!(
            workspace.join("made.txt").exists(),
            !pull_changes,
            "pull_changes={pull_changes}: the command's file is restored away only when changes were tracked"
        );
        assert_eq!(
            std::fs::read_to_string(workspace.join("keep.txt")).unwrap(),
            "kept"
        );
    }
}
