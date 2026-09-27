use prod_code_gateway::backend::BackendWorker;
use std::path::PathBuf;
#[cfg(unix)]
use std::process::ExitStatus;
use std::sync::Arc;
use std::time::Duration;
#[cfg(unix)]
use tokio::io::AsyncReadExt;

const INTERVAL: Duration = Duration::from_millis(25);
const RESPONSE_TIMEOUT: Duration = Duration::from_millis(55);
const PROBE_PREFIX: &str = "prod-code-backend-health:";

#[cfg(unix)]
struct FixtureGroup(libc::pid_t);

#[cfg(unix)]
impl FixtureGroup {
    fn new(child: &tokio::process::Child) -> Self {
        Self(child.id().expect("spawned fixture pid") as libc::pid_t)
    }

    fn retire(&self) -> std::io::Result<()> {
        let result = unsafe { libc::kill(-self.0, libc::SIGKILL) };
        if result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
}

#[cfg(unix)]
impl Drop for FixtureGroup {
    fn drop(&mut self) {
        let _ = self.retire();
    }
}

#[cfg(unix)]
struct FixtureRun {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    timed_out: bool,
}

#[cfg(unix)]
async fn finish_capture(
    task: &mut tokio::task::JoinHandle<std::io::Result<Vec<u8>>>,
    deadline: tokio::time::Instant,
    stream: &str,
) -> Result<Vec<u8>, String> {
    match tokio::time::timeout_at(deadline, &mut *task).await {
        Ok(Ok(Ok(bytes))) => Ok(bytes),
        Ok(Ok(Err(error))) => Err(format!("failed reading fixture {stream}: {error}")),
        Ok(Err(error)) => Err(format!("fixture {stream} reader failed: {error}")),
        Err(_) => {
            task.abort();
            let _ = task.await;
            Err(format!("timed out draining fixture {stream}"))
        }
    }
}

#[cfg(unix)]
async fn run_fixture_child(
    mut child: tokio::process::Child,
    group: FixtureGroup,
    watchdog: Duration,
) -> Result<FixtureRun, String> {
    const MAX_CAPTURE: u64 = 1024 * 1024;
    let stdout = child.stdout.take().ok_or("fixture stdout was not piped")?;
    let stderr = child.stderr.take().ok_or("fixture stderr was not piped")?;
    let mut stdout_task = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stdout.take(MAX_CAPTURE).read_to_end(&mut bytes).await?;
        Ok(bytes)
    });
    let mut stderr_task = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stderr.take(MAX_CAPTURE).read_to_end(&mut bytes).await?;
        Ok(bytes)
    });

    let waited = tokio::time::timeout(watchdog, child.wait()).await;
    let timed_out = waited.is_err();
    let retire_result = group.retire();
    let cleanup_deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let status = match waited {
        Ok(status) => status.map_err(|error| format!("failed waiting for fixture: {error}")),
        Err(_) => match tokio::time::timeout_at(cleanup_deadline, child.wait()).await {
            Ok(Ok(status)) => Ok(status),
            Ok(Err(error)) => Err(format!("failed reaping fixture leader: {error}")),
            Err(_) => Err("timed out reaping fixture leader".to_string()),
        },
    };
    let stdout = finish_capture(&mut stdout_task, cleanup_deadline, "stdout").await;
    let stderr = finish_capture(&mut stderr_task, cleanup_deadline, "stderr").await;
    let group_gone = tokio::time::timeout_at(cleanup_deadline, async {
        loop {
            let result = unsafe { libc::kill(-group.0, 0) };
            if result == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| "timed out retiring fixture process group".to_string());
    drop(group);
    retire_result.map_err(|error| format!("failed retiring fixture process group: {error}"))?;
    group_gone?;
    Ok(FixtureRun {
        status: status?,
        stdout: stdout?,
        stderr: stderr?,
        timed_out,
    })
}

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
    receive_matching(replies, &format!("response {id}"), |value| {
        value.get("id").and_then(serde_json::Value::as_u64) == Some(id)
    })
    .await
}

async fn receive_matching(
    replies: &mut tokio::sync::broadcast::Receiver<String>,
    description: &str,
    matches: impl Fn(&serde_json::Value) -> bool,
) -> serde_json::Value {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let message = replies.recv().await.expect("response stream remains open");
            let value: serde_json::Value = serde_json::from_str(&message).expect("response JSON");
            if matches(&value) {
                return value;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {description}"))
}

fn assert_no_private_frames(replies: &mut tokio::sync::broadcast::Receiver<String>) {
    while let Ok(message) = replies.try_recv() {
        assert!(
            !message.contains(PROBE_PREFIX),
            "private health identity leaked to a subscriber: {message}"
        );
    }
}

fn assert_only_private_server_requests(replies: &mut tokio::sync::broadcast::Receiver<String>) {
    while let Ok(message) = replies.try_recv() {
        if message.contains(PROBE_PREFIX) {
            let value: serde_json::Value =
                serde_json::from_str(&message).expect("private frame JSON");
            assert!(
                value
                    .get("method")
                    .and_then(serde_json::Value::as_str)
                    .is_some(),
                "private probe response leaked to a subscriber: {message}"
            );
        }
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
async fn fixture_watchdog_retires_pipe_holding_process_groups() {
    let workspace = tempfile::tempdir().expect("watchdog workspace");
    let descendant_pid = workspace.path().join("descendant.pid");
    let mut early = tokio::process::Command::new("sh");
    early
        .args([
            "-c",
            r#"sh -c 'trap "" TERM; printf "%s" "$$" > "$1"; while :; do sleep 1; done' sh "$1" & while [ ! -s "$1" ]; do :; done; exit 7"#,
            "sh",
        ])
        .arg(&descendant_pid)
        .process_group(0)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let early = early.spawn().expect("early-exit fixture");
    let early_group = FixtureGroup::new(&early);
    let output = run_fixture_child(early, early_group, Duration::from_millis(500))
        .await
        .expect("early-exit fixture is retired and drained");
    assert!(!output.timed_out);
    assert_eq!(output.status.code(), Some(7));
    let pid = std::fs::read_to_string(&descendant_pid)
        .expect("descendant pid")
        .parse()
        .expect("numeric descendant pid");
    wait_reaped(pid).await;

    let leader_pid = workspace.path().join("leader.pid");
    let mut stalled = tokio::process::Command::new("sh");
    stalled
        .args([
            "-c",
            r#"trap "" TERM; printf "%s" "$$" > "$1"; while :; do sleep 1; done"#,
            "sh",
        ])
        .arg(&leader_pid)
        .process_group(0)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let stalled = stalled.spawn().expect("stalled fixture");
    let stalled_group = FixtureGroup::new(&stalled);
    let output = run_fixture_child(stalled, stalled_group, Duration::from_millis(75))
        .await
        .expect("timed-out fixture is retired, reaped, and drained");
    assert!(output.timed_out);
    assert!(!output.status.success());
    let pid = std::fs::read_to_string(&leader_pid)
        .expect("leader pid")
        .parse()
        .expect("numeric leader pid");
    wait_reaped(pid).await;
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
                assert_only_private_server_requests(&mut replies);
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
            "identity" => {
                let observed = receive_matching(&mut replies, "observed health probe", |value| {
                    value["method"] == "fixture/observedProbe"
                })
                .await;
                let private_id = observed["params"]["id"]
                    .as_str()
                    .expect("fixture reports the private string id")
                    .to_owned();
                wait_until(Duration::from_millis(300), "validated health probe", || {
                    worker.health_probe_completions() >= 1
                })
                .await;

                for ordinary_id in [
                    "prod-code-backend-health:1",
                    "prod-code-backend-health:999999",
                ] {
                    worker
                        .send_lsp(
                            &serde_json::json!({
                                "jsonrpc": "2.0",
                                "id": ordinary_id,
                                "method": "fixture/ping"
                            })
                            .to_string(),
                        )
                        .await
                        .expect("legacy prefix is a legal ordinary string id");
                    let pong = receive_matching(&mut replies, "ordinary prefixed reply", |value| {
                        value["id"] == ordinary_id
                    })
                    .await;
                    assert_eq!(pong["result"], "pong");
                }

                let collision = worker
                    .send_lsp(
                        &serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": private_id,
                            "method": "fixture/ping"
                        })
                        .to_string(),
                    )
                    .await
                    .expect_err("an allocated private identity is rejected before writing");
                assert!(
                    collision.to_string().contains("health probe"),
                    "{collision:#}"
                );

                worker
                    .send_lsp(r#"{"jsonrpc":"2.0","id":700,"method":"fixture/collide"}"#)
                    .await
                    .expect("collision trigger is written");
                assert_eq!(receive_id(&mut replies, 700).await["result"], "pong");
                let request = receive_matching(
                    &mut replies,
                    "server request reusing a private identity",
                    |value| {
                        value["id"] == private_id && value["method"] == "workspace/configuration"
                    },
                )
                .await;
                assert_eq!(request["params"]["items"], serde_json::json!([]));
                receive_matching(&mut replies, "handled collision notification", |value| {
                    value["method"] == "fixture/probeCollisionHandled"
                })
                .await;
                assert!(worker.is_alive());
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
        if mode in ('success', 'flood', 'drop', 'identity'):
            send({'jsonrpc': '2.0', 'id': request['id'], 'result': None})
            if mode == 'identity' and 'actual_probe_id' not in globals():
                actual_probe_id = request['id']
                send({'jsonrpc': '2.0', 'method': 'fixture/observedProbe', 'params': {'id': actual_probe_id}})
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
    elif method == 'fixture/collide':
        send({'jsonrpc': '2.0', 'id': request['id'], 'result': 'pong'})
        send({'jsonrpc': '2.0', 'id': actual_probe_id, 'method': 'workspace/configuration', 'params': {'items': []}})
        reply = read()
        assert reply['id'] == actual_probe_id and reply['result'] == [], reply
        send({'jsonrpc': '2.0', 'method': 'fixture/probeCollisionHandled'})
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
        "identity",
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
        let group = FixtureGroup::new(&child);
        let output = run_fixture_child(child, group, Duration::from_secs(10))
            .await
            .unwrap_or_else(|error| panic!("{mode}: {error}"));
        if output.timed_out || !output.status.success() {
            failures.push(format!(
                "{mode}{}: {}{}",
                if output.timed_out {
                    " (watchdog timeout)"
                } else {
                    ""
                },
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
