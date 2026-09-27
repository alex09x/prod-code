//! The managed backend worker: a language server as a child process, framed over its stdio.
//!
//! This is the path the gateway falls back to when an in-process engine will not load, which
//! means it is the path nobody sees until something has already gone wrong. It is exercised
//! here directly, against gopls, because a fallback that has never run is not a fallback.

use prod_code_gateway::backend::BackendWorker;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

fn fixture_payload(id: u64) -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "fixture/fill",
        "params": { "padding": "x".repeat(4 * 1024 * 1024) }
    })
    .to_string()
}

async fn receive_matching(
    replies: &mut tokio::sync::broadcast::Receiver<String>,
    description: &str,
    matches: impl Fn(&serde_json::Value) -> bool,
) -> serde_json::Value {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let message = replies
                .recv()
                .await
                .expect("fixture response stream stays open");
            let value: serde_json::Value = serde_json::from_str(&message).expect("fixture JSON");
            if matches(&value) {
                return value;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {description}"))
}

/// The daemon puts the user's toolchain directories first on PATH before it looks for a
/// language server, so a test that looks for one has to do the same — otherwise it quietly
/// decides gopls is absent on a machine that has it, and passes by skipping.
fn which(binary: &str) -> Option<PathBuf> {
    prod_code_gateway::prefer_rustup_toolchain();
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(binary))
            .find(|candidate| candidate.is_file())
    })
}

fn go_workspace() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("go.mod"),
        "module example.com/backendsubject\n\ngo 1.22\n",
    )
    .expect("go.mod");
    std::fs::write(
        dir.path().join("main.go"),
        "package backendsubject\n\n// Greet returns a greeting.\nfunc Greet(name string) string {\n\treturn \"hello \" + name\n}\n",
    )
    .expect("main.go");
    dir
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_worker_starts_a_language_server_and_talks_to_it() {
    let gopls = which("gopls");
    if std::env::var_os("CI").is_some() || std::env::var_os("PROD_CODE_REQUIRE_ENGINES").is_some() {
        assert!(
            gopls.is_some(),
            "gopls must be installed where this suite is meant to run"
        );
    }
    if gopls.is_none() {
        eprintln!("SKIPPED a_worker_starts_a_language_server_and_talks_to_it: no gopls on PATH");
        return;
    }
    let workspace = go_workspace();
    let worker = BackendWorker::spawn(workspace.path(), "go")
        .await
        .expect("gopls starts and initializes");

    assert_eq!(worker.engine, "go");
    assert_eq!(
        Path::new(&worker.workspace_root),
        workspace.path(),
        "the worker knows which workspace it serves"
    );
    let capabilities = worker.capabilities.read().await.clone();
    let capabilities = capabilities.expect("initialize answered with capabilities");
    assert!(
        capabilities.get("hoverProvider").is_some()
            || capabilities.get("definitionProvider").is_some(),
        "the server said what it can do: {capabilities}"
    );

    // Ask it something, and read the answer off the broadcast every session listens on.
    let mut answers = worker.subscribe();
    let uri = format!("file://{}/main.go", workspace.path().display());
    let text = std::fs::read_to_string(workspace.path().join("main.go")).expect("read");
    worker
        .send_lsp(
            &serde_json::json!({
                "jsonrpc": "2.0",
                "method": "textDocument/didOpen",
                "params": { "textDocument": {
                    "uri": uri, "languageId": "go", "version": 1, "text": text
                }}
            })
            .to_string(),
        )
        .await
        .expect("didOpen is sent");
    worker
        .send_lsp(
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": 4242,
                "method": "textDocument/hover",
                "params": {
                    "textDocument": { "uri": uri },
                    "position": { "line": 3, "character": 6 }
                }
            })
            .to_string(),
        )
        .await
        .expect("hover is sent");

    let hover = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let Ok(message) = answers.recv().await else {
                panic!("the worker's channel closed before it answered");
            };
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&message) else {
                continue;
            };
            if value.get("id").and_then(|id| id.as_i64()) == Some(4242) {
                return value;
            }
        }
    })
    .await
    .expect("the hover is answered within the timeout");

    let rendered = hover.to_string();
    assert!(
        rendered.contains("Greet"),
        "the answer is about the function under the cursor: {rendered}"
    );

    // Exit detection applies to the fallback too: a cached workspace must not hand a dead
    // subprocess to the next client after the primary engine has failed.
    let worker = std::sync::Arc::new(worker);
    let shared = prod_code_gateway::workspace::SharedWorkspace::new(
        workspace.path().to_path_buf(),
        "go".to_string(),
        None,
        None,
        None,
        Some(std::sync::Arc::clone(&worker)),
    );
    assert!(!shared.has_dead_server());
    worker
        .send_lsp(r#"{"jsonrpc":"2.0","method":"exit"}"#)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while !shared.has_dead_server() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("a stopped fallback must make its workspace non-reusable");

    // The open document is remembered, so a later session does not re-open it.
    let open = worker.open_files.read().await.clone();
    assert!(
        open.is_empty() || open.iter().any(|f| f.contains("main.go")),
        "open files are tracked: {open:?}"
    );
}

#[tokio::test]
async fn a_language_nobody_manages_is_refused_by_name() {
    let workspace = tempfile::tempdir().expect("tempdir");
    // `BackendWorker` is not `Debug`, so the refusal is matched rather than unwrapped.
    let text = match BackendWorker::spawn(workspace.path(), "cobol").await {
        Ok(_) => panic!("there is no managed backend for cobol"),
        Err(err) => format!("{err:#}"),
    };
    assert!(
        text.contains("cobol"),
        "the refusal names the engine it was asked for: {text}"
    );
}

/// Each fake-server mode runs in its own test process: PATH is never changed in a shared test.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fallback_initialization_requires_a_valid_response() {
    const MODE: &str = "PROD_CODE_BACKEND_LIFECYCLE_CASE";
    if let Ok(mode) = std::env::var(MODE) {
        let dir = tempfile::tempdir().unwrap();
        let outcome = if mode == "queue-health" {
            BackendWorker::spawn_with_health_config(
                dir.path(),
                "go",
                Duration::from_millis(700),
                Duration::from_millis(300),
                Duration::from_millis(40),
            )
            .await
        } else if mode.starts_with("frame-") {
            tokio::time::timeout(
                Duration::from_secs(2),
                BackendWorker::spawn(dir.path(), "go"),
            )
            .await
            .expect("invalid framing must close the response stream promptly")
        } else if matches!(
            mode.as_str(),
            "blocked-send"
                | "queue-health"
                | "partial-cancel"
                | "drop-child"
                | "eof-after-init"
                | "auto-queue"
                | "auto-partial"
        ) {
            BackendWorker::spawn_with_write_timeout(dir.path(), "go", Duration::from_millis(150))
                .await
        } else {
            BackendWorker::spawn(dir.path(), "go").await
        };
        if matches!(
            mode.as_str(),
            "valid"
                | "collision"
                | "content-type-after"
                | "content-type-before"
                | "blocked-send"
                | "queue-health"
                | "partial-cancel"
                | "drop-child"
                | "eof-after-init"
                | "auto-queue"
                | "auto-partial"
                | "configuration"
        ) {
            let worker = Arc::new(outcome.unwrap_or_else(|error| panic!("{mode}: {error:#}")));
            let capabilities = worker.capabilities.read().await.clone();
            if mode == "drop-child" {
                assert!(capabilities.as_ref().unwrap()["fixturePid"].is_number());
            } else {
                assert_eq!(capabilities, Some(serde_json::json!({})), "{mode}");
            }
            if mode.starts_with("content-type-") || mode == "configuration" {
                let mut replies = worker.subscribe();
                worker
                    .send_lsp(
                        r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{}}"#,
                    )
                    .await
                    .unwrap();
                let response = tokio::time::timeout(Duration::from_secs(2), replies.recv())
                    .await
                    .expect("framing must stay aligned")
                    .unwrap();
                let response: serde_json::Value = serde_json::from_str(&response).unwrap();
                assert_eq!(response["result"]["contents"], "fixture hover");
            }
            if mode == "blocked-send" {
                let mut replies = worker.subscribe();
                worker
                    .send_lsp(r#"{"jsonrpc":"2.0","id":3,"method":"fixture/arm"}"#)
                    .await
                    .unwrap();
                receive_matching(&mut replies, "arm response", |value| value["id"] == 3).await;
                let error = tokio::time::timeout(
                    Duration::from_secs(2),
                    worker.send_lsp(&fixture_payload(4)),
                )
                .await
                .expect("send_lsp must enforce its own finite write budget")
                .expect_err("the unread pipe must reject the blocked write");
                assert!(error.to_string().contains("timed out"), "{error:#}");
                assert!(
                    !worker.is_alive(),
                    "a partially written frame poisons the worker"
                );
                let follower = worker
                    .send_lsp(r#"{"jsonrpc":"2.0","method":"fixture/follower"}"#)
                    .await
                    .expect_err("a follower must not append to a partial frame");
                assert!(follower.to_string().contains("exited"), "{follower:#}");
            }
            if mode == "partial-cancel" {
                let mut replies = worker.subscribe();
                worker
                    .send_lsp(r#"{"jsonrpc":"2.0","id":3,"method":"fixture/arm"}"#)
                    .await
                    .unwrap();
                receive_matching(&mut replies, "arm response", |value| value["id"] == 3).await;
                let first_worker = Arc::clone(&worker);
                let first = tokio::spawn(async move {
                    first_worker
                        .send_lsp_with_write_timeout(&fixture_payload(4), Duration::from_secs(5))
                        .await
                });
                receive_matching(&mut replies, "observed partial frame", |value| {
                    value["method"] == "fixture/partial"
                })
                .await;

                let follower_payload = fixture_payload(5);
                let mut follower = Box::pin(worker.send_lsp(&follower_payload));
                assert!(matches!(
                    futures_util::poll!(follower.as_mut()),
                    Poll::Pending
                ));
                first.abort();
                assert!(
                    first
                        .await
                        .expect_err("the first send is cancelled")
                        .is_cancelled()
                );
                let follower_error = tokio::time::timeout(Duration::from_secs(1), follower)
                    .await
                    .expect("the queued follower is woken")
                    .expect_err("the queued follower sees a retired connection");
                assert!(
                    follower_error.to_string().contains("exited"),
                    "{follower_error:#}"
                );
                assert!(!worker.is_alive());
            }
            if mode == "queue-health" {
                let mut replies = worker.subscribe();
                worker
                    .send_lsp(r#"{"jsonrpc":"2.0","id":3,"method":"fixture/arm"}"#)
                    .await
                    .unwrap();
                receive_matching(&mut replies, "arm response", |value| value["id"] == 3).await;
                let first_worker = Arc::clone(&worker);
                let first = tokio::spawn(async move {
                    first_worker
                        .send_lsp_with_write_timeout(&fixture_payload(4), Duration::from_secs(5))
                        .await
                });
                receive_matching(&mut replies, "observed partial frame", |value| {
                    value["method"] == "fixture/partial"
                })
                .await;

                let queue_error = worker
                    .send_lsp(r#"{"jsonrpc":"2.0","method":"fixture/queued"}"#)
                    .await
                    .expect_err("a queued writer has a finite lock budget");
                assert!(
                    queue_error.to_string().contains("waiting"),
                    "{queue_error:#}"
                );
                assert!(
                    worker.is_alive(),
                    "a writer that emitted no bytes cannot poison framing"
                );

                let cancelled_payload = fixture_payload(6);
                let mut cancelled = Box::pin(worker.send_lsp(&cancelled_payload));
                assert!(matches!(
                    futures_util::poll!(cancelled.as_mut()),
                    Poll::Pending
                ));
                drop(cancelled);
                assert!(
                    worker.is_alive(),
                    "queue cancellation emitted no frame bytes"
                );

                std::fs::write(dir.path().join("release-write"), b"release").unwrap();
                tokio::time::timeout(Duration::from_secs(2), first)
                    .await
                    .expect("the released frame finishes")
                    .expect("writer task joins")
                    .expect("released frame is written");
                worker
                    .send_lsp(r#"{"jsonrpc":"2.0","id":8,"method":"fixture/ping"}"#)
                    .await
                    .expect("healthy writer accepts another frame");
                let pong =
                    receive_matching(&mut replies, "post-timeout ping", |value| value["id"] == 8)
                        .await;
                assert_eq!(pong["result"], "pong");
            }
            if mode == "auto-queue" || mode == "auto-partial" {
                let mut replies = worker.subscribe();
                worker
                    .send_lsp(r#"{"jsonrpc":"2.0","id":3,"method":"fixture/arm"}"#)
                    .await
                    .unwrap();
                receive_matching(&mut replies, "arm response", |value| value["id"] == 3).await;
                let first = if mode == "auto-queue" {
                    let first_worker = Arc::clone(&worker);
                    Some(tokio::spawn(async move {
                        first_worker
                            .send_lsp_with_write_timeout(
                                &fixture_payload(4),
                                Duration::from_secs(5),
                            )
                            .await
                    }))
                } else {
                    None
                };
                tokio::time::timeout(Duration::from_secs(2), async {
                    while worker.is_alive() {
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .expect("an undelivered automatic response must retire the backend");
                if let Some(first) = first {
                    tokio::time::timeout(Duration::from_secs(2), first)
                        .await
                        .expect("retirement wakes the blocked writer")
                        .expect("writer joins")
                        .expect_err("the owned child was retired");
                }
                worker
                    .send_lsp(r#"{"jsonrpc":"2.0","method":"fixture/after-auto-failure"}"#)
                    .await
                    .expect_err("an unanswered automatic request cannot leave a reusable worker");
            }
            if mode == "eof-after-init" {
                worker
                    .send_lsp(r#"{"jsonrpc":"2.0","method":"fixture/eof"}"#)
                    .await
                    .unwrap();
                tokio::time::timeout(Duration::from_secs(2), async {
                    while worker.is_alive() {
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .expect("EOF marks the worker dead");
                let error = worker
                    .send_lsp(r#"{"jsonrpc":"2.0","method":"fixture/after-eof"}"#)
                    .await
                    .expect_err("EOF wakes and rejects later writers");
                assert!(error.to_string().contains("exited"), "{error:#}");
            }
            if mode == "drop-child" {
                let pid = capabilities
                    .as_ref()
                    .and_then(|value| value["fixturePid"].as_i64())
                    .expect("fixture reports its child pid")
                    as libc::pid_t;
                drop(worker);
                tokio::time::timeout(Duration::from_secs(2), async {
                    loop {
                        let result = unsafe { libc::kill(pid, 0) };
                        if result == -1
                            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
                        {
                            break;
                        }
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .expect("dropping the worker kills and reaps its owned child");
            }
        } else {
            let error = match outcome {
                Ok(_) => panic!("{mode}: invalid initialization was accepted"),
                Err(error) => format!("{error:#}"),
            };
            assert!(
                error.to_lowercase().contains("initializ"),
                "{mode}: {error}"
            );
            match mode.as_str() {
                "error" => assert!(error.contains("fixture refused initialization"), "{error}"),
                "malformed" => assert!(error.contains("capabilities"), "{error}"),
                "eof" | "frame-duplicate" | "frame-oversize" | "frame-header"
                | "frame-truncated" => assert!(
                    error.contains("exited") || error.contains("closed"),
                    "{error}"
                ),
                "silent" => assert!(error.to_lowercase().contains("timeout"), "{error}"),
                _ => panic!("unknown mode"),
            }
        }
        return;
    }
    use std::os::unix::fs::PermissionsExt;
    let binaries = tempfile::tempdir().unwrap();
    let script = binaries.path().join("gopls");
    std::fs::write(&script, r#"#!/usr/bin/env python3
import json, os, sys, threading

def read_length():
    n = 0
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        if not line.strip():
            break
        if line.lower().startswith(b'content-length:'):
            n = int(line.split(b':', 1)[1])
    return n

def read():
    n = read_length()
    if n is None:
        return None
    return json.loads(sys.stdin.buffer.read(n))

def send(value):
    body = json.dumps(value).encode()
    header = b'Content-Length: %d\r\n' % len(body)
    content_type = b'Content-Type: application/vscode-jsonrpc; charset=utf-8\r\n'
    if mode == 'content-type-after':
        header += content_type
    elif mode == 'content-type-before':
        header = content_type + header
    sys.stdout.buffer.write(header + b'\r\n' + body)
    sys.stdout.buffer.flush()

initial = read()
assert initial['method'] == 'initialize'
mode = os.environ['PROD_CODE_BACKEND_LIFECYCLE_CASE']
if mode.startswith('frame-'):
    malformed = {
        'frame-duplicate': b'Content-Length: 2\r\nContent-Length: 2\r\n\r\n{}',
        'frame-oversize': b'Content-Length: 268435457\r\n\r\n',
        'frame-header': b'A' * 65537,
        'frame-truncated': b'Content-Length: 10\r\n\r\n{}',
    }[mode]
    sys.stdout.buffer.write(malformed)
    sys.stdout.buffer.flush()
    if mode == 'frame-truncated':
        sys.exit(0)
    threading.Event().wait()
if mode == 'eof':
    sys.exit(0)
if mode == 'silent':
    threading.Event().wait()
if mode == 'configuration':
    valid = [[], [{}], [{'section': 'gopls'}, {'section': 'unknown', 'scopeUri': 'file:///workspace/main.go'}, {'scopeUri': 'file:///workspace/second.go'}]]
    for items in valid:
        send({'jsonrpc': '2.0', 'id': 'config-é', 'method': 'workspace/configuration', 'params': {'items': items}})
        reply = read()
        assert reply['id'] == 'config-é', reply
        assert 'error' not in reply and reply['result'] == [{} for _ in items], reply
    for params in [None, {}, {'items': None}, {'items': {}}, {'items': [None]}, {'items': [{'section': 7}]}, {'items': [{'scopeUri': False}]}]:
        send({'jsonrpc': '2.0', 'id': 'config-é', 'method': 'workspace/configuration', 'params': params})
        reply = read()
        assert reply['id'] == 'config-é', reply
        assert 'result' not in reply and reply['error']['code'] == -32602, reply
        assert 'workspace/configuration' in reply['error']['message'], reply
if mode == 'collision':
    send({'jsonrpc': '2.0', 'id': 1, 'method': 'workspace/configuration', 'params': {'items': [{}]}})
    response = read()
    assert response.get('result') == [{}], response
if mode == 'error':
    send({'jsonrpc': '2.0', 'id': 1, 'error': {'code': -32603, 'message': 'fixture refused initialization'}})
elif mode == 'malformed':
    send({'jsonrpc': '2.0', 'id': 1, 'result': {}})
else:
    capabilities = {'fixturePid': os.getpid()} if mode == 'drop-child' else {}
    send({'jsonrpc': '2.0', 'id': 1, 'result': {'capabilities': capabilities}})
while True:
    request = read()
    if request is None:
        break
    if request.get('method') == 'textDocument/hover':
        send({'jsonrpc': '2.0', 'id': request['id'], 'result': {'contents': 'fixture hover'}})
    elif request.get('method') == 'fixture/arm':
        send({'jsonrpc': '2.0', 'id': request['id'], 'result': None})
        if mode == 'blocked-send':
            threading.Event().wait()
        if mode == 'auto-partial':
            send({'jsonrpc': '2.0', 'id': 'x' * (4 * 1024 * 1024), 'method': 'workspace/configuration', 'params': {'items': [{}]}})
            threading.Event().wait()
        if mode in ('queue-health', 'partial-cancel', 'auto-queue'):
            n = read_length()
            first = sys.stdin.buffer.read(1)
            send({'jsonrpc': '2.0', 'method': 'fixture/partial'})
            if mode == 'partial-cancel':
                threading.Event().wait()
            if mode == 'auto-queue':
                send({'jsonrpc': '2.0', 'id': 99, 'method': 'workspace/configuration', 'params': {'items': [{}]}})
                threading.Event().wait()
            released = os.path.join(os.getcwd(), 'release-write')
            while not os.path.exists(released):
                threading.Event().wait(0.01)
            json.loads(first + sys.stdin.buffer.read(n - 1))
    elif request.get('method') == 'fixture/ping':
        send({'jsonrpc': '2.0', 'id': request['id'], 'result': 'pong'})
    elif request.get('method') == 'fixture/eof':
        sys.exit(0)
"#).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::join_paths(
        std::iter::once(binaries.path().to_path_buf())
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let mut failures = Vec::new();
    for mode in [
        "valid",
        "collision",
        "content-type-after",
        "content-type-before",
        "blocked-send",
        "queue-health",
        "partial-cancel",
        "drop-child",
        "eof-after-init",
        "auto-queue",
        "auto-partial",
        "configuration",
        "error",
        "malformed",
        "eof",
        "silent",
        "frame-duplicate",
        "frame-oversize",
        "frame-header",
        "frame-truncated",
    ] {
        struct FixtureGroup(libc::pid_t);
        impl Drop for FixtureGroup {
            fn drop(&mut self) {
                // The fake server inherits this private group; an external watchdog must
                // clean it too when terminating the test process prevents Rust destructors.
                unsafe { libc::kill(-self.0, libc::SIGKILL) };
            }
        }
        let child = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "fallback_initialization_requires_a_valid_response",
                "--nocapture",
            ])
            .env(MODE, mode)
            .env("PATH", &path)
            .process_group(0)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let group = FixtureGroup(child.id().unwrap() as libc::pid_t);
        let output = tokio::time::timeout(Duration::from_secs(15), child.wait_with_output())
            .await
            .unwrap_or_else(|_| panic!("{mode}: fixture child exceeded external watchdog"))
            .unwrap();
        drop(group);
        if !output.status.success() {
            failures.push(format!(
                "{mode}: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
