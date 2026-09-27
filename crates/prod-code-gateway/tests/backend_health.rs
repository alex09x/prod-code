use prod_code_gateway::backend::BackendWorker;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

const INTERVAL: Duration = Duration::from_millis(25);
const RESPONSE_TIMEOUT: Duration = Duration::from_millis(55);
const PROBE_PREFIX: &str = "prod-code-backend-health:";

async fn wait_until(deadline: Duration, description: &str, mut ready: impl FnMut() -> bool) {
    tokio::time::timeout(deadline, async {
        while !ready() {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {description}"));
}

#[cfg(unix)]
async fn wait_reaped(pid: libc::pid_t) {
    wait_until(Duration::from_secs(2), "owned child to be reaped", || {
        let result = unsafe { libc::kill(pid, 0) };
        result == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    })
    .await;
}

async fn receive_id(
    replies: &mut tokio::sync::broadcast::Receiver<String>,
    id: u64,
) -> serde_json::Value {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let message = replies.recv().await.expect("response stream remains open");
            let value: serde_json::Value = serde_json::from_str(&message).expect("response JSON");
            if value.get("id").and_then(serde_json::Value::as_u64) == Some(id) {
                return value;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for response {id}"))
}

fn assert_no_private_frames(replies: &mut tokio::sync::broadcast::Receiver<String>) {
    while let Ok(message) = replies.try_recv() {
        assert!(
            !message.contains(PROBE_PREFIX),
            "private health identity leaked to a subscriber: {message}"
        );
    }
}

async fn controlled_worker(workspace: &std::path::Path) -> Arc<BackendWorker> {
    Arc::new(
        BackendWorker::spawn_with_health_config(
            workspace,
            "go",
            Duration::from_secs(2),
            INTERVAL,
            RESPONSE_TIMEOUT,
        )
        .await
        .expect("controlled backend initializes"),
    )
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn controlled_backends_prove_health_lifecycle_and_private_routing() {
    const MODE_ENV: &str = "PROD_CODE_BACKEND_HEALTH_CASE";
    if let Ok(mode) = std::env::var(MODE_ENV) {
        let workspace = tempfile::tempdir().expect("workspace");
        if mode == "cancel-init" {
            let path = workspace.path().to_path_buf();
            let task = tokio::spawn(async move {
                BackendWorker::spawn_with_health_config(
                    &path,
                    "go",
                    Duration::from_secs(2),
                    INTERVAL,
                    RESPONSE_TIMEOUT,
                )
                .await
            });
            let pid_file = workspace.path().join("fixture.pid");
            wait_until(Duration::from_secs(2), "initializing fixture pid", || {
                pid_file.is_file()
            })
            .await;
            let pid: libc::pid_t = std::fs::read_to_string(pid_file)
                .expect("pid")
                .parse()
                .expect("numeric pid");
            task.abort();
            assert!(
                matches!(task.await, Err(error) if error.is_cancelled()),
                "initialization task must report cancellation"
            );
            wait_reaped(pid).await;
            return;
        }

        let worker = controlled_worker(workspace.path()).await;
        let pid = worker.process_id().expect("owned child pid") as libc::pid_t;
        let mut replies = worker.subscribe();
        match mode.as_str() {
            "silence" => {
                let shared = prod_code_gateway::workspace::SharedWorkspace::new(
                    workspace.path().to_path_buf(),
                    "go".to_string(),
                    None,
                    None,
                    None,
                    Some(Arc::clone(&worker)),
                );
                assert!(!shared.has_dead_server());
                wait_until(Duration::from_millis(500), "three silent probes", || {
                    !worker.is_alive()
                })
                .await;
                assert!(shared.has_dead_server());
                assert!(worker.capabilities.read().await.is_none());
                assert_eq!(worker.health_probe_completions(), 0);
            }
            "success" | "error" => {
                wait_until(Duration::from_millis(500), "five valid probes", || {
                    worker.health_probe_completions() >= 5
                })
                .await;
                assert!(worker.is_alive());
                assert!(worker.retained_health_responses() <= 1);
                assert_no_private_frames(&mut replies);
            }
            "late" => {
                wait_until(Duration::from_millis(600), "validated late replies", || {
                    worker.health_probe_completions() >= 4
                })
                .await;
                assert!(worker.is_alive(), "late valid replies reset timeout epochs");
                assert_no_private_frames(&mut replies);
            }
            "malformed" => {
                wait_until(
                    Duration::from_millis(500),
                    "malformed probe retirement",
                    || !worker.is_alive(),
                )
                .await;
                assert_eq!(worker.health_probe_completions(), 0);
                assert_no_private_frames(&mut replies);
            }
            "activity" => {
                for id in 10..18 {
                    worker
                        .send_lsp(
                            &serde_json::json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "method": "fixture/ping"
                            })
                            .to_string(),
                        )
                        .await
                        .expect("ordinary request");
                    assert_eq!(receive_id(&mut replies, id).await["result"], "pong");
                    tokio::time::sleep(Duration::from_millis(15)).await;
                }
                assert!(worker.is_alive(), "ordinary traffic defers idle probes");
                worker
                    .send_lsp(r#"{"jsonrpc":"2.0","id":99,"method":"fixture/lost"}"#)
                    .await
                    .expect("lost request is written");
                wait_until(
                    Duration::from_millis(500),
                    "lost request not retained as busy",
                    || !worker.is_alive(),
                )
                .await;
            }
            "flood" => {
                for id in 1000..1300 {
                    worker
                        .send_lsp(
                            &serde_json::json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "method": "fixture/ping"
                            })
                            .to_string(),
                        )
                        .await
                        .expect("ordinary request");
                    assert_eq!(receive_id(&mut replies, id).await["result"], "pong");
                }
                assert!(worker.retained_dispatch_identities() <= 256);
                wait_until(Duration::from_secs(2), "many bounded probes", || {
                    worker.health_probe_completions() >= 20
                })
                .await;
                assert!(worker.retained_health_responses() <= 1);
                assert_no_private_frames(&mut replies);
            }
            "drop" => {}
            other => panic!("unknown fixture mode {other}"),
        }
        drop(worker);
        wait_reaped(pid).await;
        return;
    }

    use std::os::unix::fs::PermissionsExt;
    let binaries = tempfile::tempdir().expect("binaries");
    let script = binaries.path().join("gopls");
    std::fs::write(
        &script,
        r#"#!/usr/bin/env python3
import json, os, sys

mode = os.environ['PROD_CODE_BACKEND_HEALTH_CASE']
with open(os.path.join(os.getcwd(), 'fixture.pid'), 'w') as pid_file:
    pid_file.write(str(os.getpid()))

def read():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        if not line.strip():
            break
        if line.lower().startswith(b'content-length:'):
            length = int(line.split(b':', 1)[1])
    if length is None:
        return None
    return json.loads(sys.stdin.buffer.read(length))

def send(value):
    body = json.dumps(value).encode()
    sys.stdout.buffer.write(b'Content-Length: %d\r\n\r\n' % len(body) + body)
    sys.stdout.buffer.flush()

request = read()
assert request['method'] == 'initialize'
if mode == 'cancel-init':
    while read() is not None:
        pass
    sys.exit(0)
send({'jsonrpc': '2.0', 'id': request['id'], 'result': {'capabilities': {}}})
late = None
while True:
    request = read()
    if request is None:
        break
    method = request.get('method')
    if method == 'prodCode/healthProbe':
        if mode in ('success', 'flood', 'drop'):
            send({'jsonrpc': '2.0', 'id': request['id'], 'result': None})
        elif mode == 'error':
            send({'jsonrpc': '2.0', 'id': request['id'], 'error': {'code': -32601, 'message': 'unknown method'}})
        elif mode == 'late':
            if late is not None:
                send({'jsonrpc': '2.0', 'id': late, 'error': {'code': -32601, 'message': 'late unknown method'}})
            late = request['id']
        elif mode == 'malformed':
            send({'jsonrpc': '2.0', 'id': request['id'], 'method': 'workspace/configuration', 'params': {'items': []}})
            reply = read()
            assert reply['id'] == request['id'] and reply['result'] == [], reply
            send({'jsonrpc': '2.0', 'id': request['id'], 'result': None, 'error': {'code': -1, 'message': 'both'}})
    elif method == 'fixture/ping':
        send({'jsonrpc': '2.0', 'id': request['id'], 'result': 'pong'})
    elif method == 'fixture/lost':
        pass
"#,
    )
    .expect("script");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("executable");
    let path = std::env::join_paths(std::iter::once(binaries.path().to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").expect("PATH")),
    ))
    .expect("PATH");

    let mut failures = Vec::new();
    for mode in [
        "silence",
        "success",
        "error",
        "late",
        "malformed",
        "activity",
        "flood",
        "drop",
        "cancel-init",
    ] {
        let child = tokio::process::Command::new(std::env::current_exe().expect("test binary"))
            .args([
                "--exact",
                "controlled_backends_prove_health_lifecycle_and_private_routing",
                "--nocapture",
            ])
            .env(MODE_ENV, mode)
            .env("PATH", &path)
            .process_group(0)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("fixture test child");
        let group = child.id().expect("pid") as libc::pid_t;
        let output = tokio::time::timeout(Duration::from_secs(10), child.wait_with_output())
            .await
            .unwrap_or_else(|_| panic!("{mode}: fixture test exceeded watchdog"))
            .expect("fixture output");
        unsafe { libc::kill(-group, libc::SIGKILL) };
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

fn which(binary: &str) -> Option<PathBuf> {
    prod_code_gateway::prefer_rustup_toolchain();
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|directory| directory.join(binary))
            .find(|candidate| candidate.is_file())
    })
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_gopls_answers_before_and_after_scheduled_probes_without_touching_source() {
    let gopls = which("gopls");
    if std::env::var_os("CI").is_some() || std::env::var_os("PROD_CODE_REQUIRE_ENGINES").is_some() {
        assert!(gopls.is_some(), "gopls is required on this test node");
    }
    if gopls.is_none() {
        eprintln!(
            "SKIPPED real_gopls_answers_before_and_after_scheduled_probes_without_touching_source: no gopls"
        );
        return;
    }

    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::write(
        workspace.path().join("go.mod"),
        "module example.com/backendhealth\n\ngo 1.22\n",
    )
    .expect("go.mod");
    let source = b"package backendhealth\n\n// Greet returns a greeting.\nfunc Greet(name string) string {\n\treturn \"hello \" + name\n}\n";
    let source_path = workspace.path().join("main.go");
    std::fs::write(&source_path, source).expect("source");
    let worker = BackendWorker::spawn_with_health_config(
        workspace.path(),
        "go",
        Duration::from_secs(30),
        Duration::from_millis(75),
        Duration::from_secs(2),
    )
    .await
    .expect("gopls initializes");
    let pid = worker.process_id().expect("gopls pid") as libc::pid_t;
    let mut replies = worker.subscribe();
    let uri = format!("file://{}", source_path.display());
    worker
        .send_lsp(
            &serde_json::json!({
                "jsonrpc": "2.0",
                "method": "textDocument/didOpen",
                "params": {"textDocument": {
                    "uri": uri, "languageId": "go", "version": 1,
                    "text": String::from_utf8_lossy(source)
                }}
            })
            .to_string(),
        )
        .await
        .expect("didOpen");
    for id in [7001_u64, 7002] {
        if id == 7002 {
            wait_until(
                Duration::from_secs(10),
                "two validated gopls probes",
                || worker.health_probe_completions() >= 2,
            )
            .await;
        }
        worker
            .send_lsp(
                &serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "method": "textDocument/hover",
                    "params": {
                        "textDocument": {"uri": uri},
                        "position": {"line": 3, "character": 6}
                    }
                })
                .to_string(),
            )
            .await
            .expect("hover request");
        let hover = receive_id(&mut replies, id).await;
        assert!(hover.to_string().contains("Greet"), "hover {id}: {hover}");
    }
    assert_eq!(
        std::fs::read(&source_path).expect("source after probes"),
        source
    );
    assert_no_private_frames(&mut replies);
    drop(worker);
    wait_reaped(pid).await;
}
