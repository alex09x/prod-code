#![cfg(unix)]

use futures_util::{SinkExt, StreamExt};
use prod_code_gateway::editor_proxy::{
    EditorProxyOptions, EditorServers, HEALTH_PROBE_ID_PREFIX, HEALTH_PROBE_METHOD, ProbeState,
    ServerCommand, run_with_options, server_command,
};
use prod_code_protocol::{PathTranslator, ProdCodeCodec, WireMessage, readiness::ReadySignal};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio_util::codec::Framed;

const WAIT: Duration = Duration::from_secs(5);
const SHORT_WRITE: Duration = Duration::from_millis(300);
const SHORT_TEARDOWN: Duration = Duration::from_secs(1);

struct Session {
    editor: Option<Framed<TcpStream, ProdCodeCodec>>,
    task: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
}

impl Session {
    async fn finish(mut self) -> Result<(), String> {
        self.editor.take();
        let task = self.task.as_mut().expect("session task is owned");
        let outcome = match tokio::time::timeout(WAIT, &mut *task).await {
            Ok(Ok(Ok(()))) => Ok(()),
            Ok(Ok(Err(error))) => Err(format!("editor session failed: {error:#}")),
            Ok(Err(error)) => Err(format!("editor session task failed: {error}")),
            Err(_) => {
                task.abort();
                let cancelled = (&mut *task).await;
                Err(format!(
                    "editor session timed out; forced cancellation result: {cancelled:?}"
                ))
            }
        };
        self.task.take();
        outcome
    }

    async fn cancel(mut self) -> Result<(), String> {
        self.editor.take();
        let task = self.task.as_mut().expect("session task is owned");
        task.abort();
        let outcome = match tokio::time::timeout(WAIT, &mut *task).await {
            Ok(Err(error)) if error.is_cancelled() => Ok(()),
            other => Err(format!("editor session did not cancel cleanly: {other:?}")),
        };
        if task.is_finished() {
            self.task.take();
        }
        outcome
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

fn fake_server_script() -> &'static str {
    r#"
import sys, json, os, time

pid = os.getpid()
pid_file = os.environ.get("EDITOR_PID_FILE")
if pid_file:
    with open(pid_file, "w") as f:
        f.write(str(pid))

log_file = os.environ.get("EDITOR_LOG_FILE")
ignore_probes = os.environ.get("IGNORE_PROBES") == "1"

def log(method, msg_id):
    if log_file:
        with open(log_file, "a") as f:
            f.write(f"{method}:{msg_id}\n")

def send(obj):
    body = json.dumps(obj)
    sys.stdout.write(f"Content-Length: {len(body)}\r\n\r\n{body}")
    sys.stdout.flush()

while True:
    line = sys.stdin.readline()
    if not line:
        break
    if line.startswith("Content-Length:"):
        length = int(line.split(":")[1].strip())
        while True:
            empty = sys.stdin.readline()
            if empty in ("\r\n", "\n"):
                break
        body = sys.stdin.read(length)
        msg = json.loads(body)
        method = msg.get("method")
        msg_id = msg.get("id")
        log(method, msg_id)

        if method == "prodCode/healthProbe":
            if not ignore_probes:
                send({"jsonrpc": "2.0", "id": msg_id, "result": {"status": "ok"}})
        elif method == "ping":
            send({"jsonrpc": "2.0", "id": msg_id, "result": "pong"})
        elif method == "slowWork":
            delay = msg.get("params", {}).get("delay_ms", 0)
            if delay > 0:
                time.sleep(delay / 1000.0)
            send({"jsonrpc": "2.0", "id": msg_id, "result": "done"})
        elif method == "startProgress":
            token = msg.get("params", {}).get("token", "idx")
            send({"jsonrpc": "2.0", "method": "window/workDoneProgress/create", "params": {"token": token}})
            send({"jsonrpc": "2.0", "method": "$/progress", "params": {"token": token, "value": {"kind": "begin", "title": "indexing"}}})
            send({"jsonrpc": "2.0", "id": msg_id, "result": "progress_started"})
        elif method == "endProgress":
            token = msg.get("params", {}).get("token", "idx")
            send({"jsonrpc": "2.0", "method": "$/progress", "params": {"token": token, "value": {"kind": "end"}}})
            send({"jsonrpc": "2.0", "id": msg_id, "result": "progress_ended"})
        elif msg_id is not None and method is not None:
            send({"jsonrpc": "2.0", "id": msg_id, "result": None})
"#
}

fn python_command(
    root: &Path,
    pid_file: &Path,
    log_file: &Path,
    ignore_probes: bool,
    ready: ReadySignal,
) -> ServerCommand {
    let script = root.join("fake_editor_server.py");
    std::fs::write(&script, fake_server_script()).expect("write fake editor server");
    ServerCommand {
        program: "python3".to_string(),
        args: vec![script.to_string_lossy().into_owned()],
        env: vec![
            (
                "EDITOR_PID_FILE".to_string(),
                pid_file.to_string_lossy().into_owned(),
            ),
            (
                "EDITOR_LOG_FILE".to_string(),
                log_file.to_string_lossy().into_owned(),
            ),
            (
                "IGNORE_PROBES".to_string(),
                if ignore_probes { "1" } else { "0" }.to_string(),
            ),
        ],
        ready,
    }
}

async fn start_session_with_options(
    root: &Path,
    command: ServerCommand,
    servers: Arc<EditorServers>,
    options: EditorProxyOptions,
) -> Session {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind proxy");
    let address = listener.local_addr().expect("proxy address");
    let session_root = root.to_path_buf();
    let session_servers = Arc::clone(&servers);
    let task = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.expect("accept editor");
        run_with_options(
            Framed::new(socket, ProdCodeCodec::new()),
            PathTranslator::new("/client", &session_root.to_string_lossy()),
            command,
            &session_root,
            session_servers.as_ref(),
            574,
            options,
        )
        .await
    });
    let mut session = Session {
        editor: None,
        task: Some(task),
    };
    match tokio::time::timeout(WAIT, TcpStream::connect(address)).await {
        Ok(Ok(socket)) => {
            session.editor = Some(Framed::new(socket, ProdCodeCodec::new()));
        }
        Ok(Err(error)) => {
            let cleanup = session.cancel().await;
            panic!("connect editor: {error}; task cleanup: {cleanup:?}");
        }
        Err(_) => {
            let cleanup = session.cancel().await;
            panic!("connect editor timed out; task cleanup: {cleanup:?}");
        }
    }
    session
}

async fn wait_for_pid(path: &Path) -> Result<i32, String> {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        if let Ok(text) = std::fs::read_to_string(path) {
            if let Ok(pid) = text.trim().parse() {
                return Ok(pid);
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("fake editor server did not publish PID".to_string());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn wait_for_log_count(path: &Path, method: &str, count: usize) -> Result<Vec<String>, String> {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        if let Ok(text) = std::fs::read_to_string(path) {
            let matching: Vec<String> = text
                .lines()
                .filter(|line| line.starts_with(method))
                .map(|line| line.to_string())
                .collect();
            if matching.len() >= count {
                return Ok(matching);
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!("did not see {count} occurrences of {method} in log"));
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn wait_for_exit(pid: i32) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        if unsafe { libc::kill(pid, 0) } == -1
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
        {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            let _ = unsafe { libc::kill(pid, libc::SIGKILL) };
            return Err(format!("process {pid} survived cleanup"));
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn editor_health_probes_reach_server_and_replies_are_withheld_from_editor() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    let pid_file = root.join("server.pid");
    let log_file = root.join("server.log");

    let probe_state = Arc::new(std::sync::Mutex::new(ProbeState::default()));
    let command = python_command(
        root,
        &pid_file,
        &log_file,
        false,
        ReadySignal::default(),
    );
    let servers = Arc::new(EditorServers::default());
    let options = EditorProxyOptions {
        write_budget: SHORT_WRITE,
        teardown_budget: SHORT_TEARDOWN,
        health_probe_interval: Some(Duration::from_millis(40)),
        health_response_timeout: Duration::from_millis(50),
        probe_state: Some(Arc::clone(&probe_state)),
    };

    let mut session = start_session_with_options(root, command, servers, options).await;
    let pid = wait_for_pid(&pid_file).await.expect("pid");

    // Wait for at least 2 health probes to reach the fake server
    let probes = wait_for_log_count(&log_file, HEALTH_PROBE_METHOD, 2)
        .await
        .expect("probes in log");
    assert!(probes.len() >= 2);
    for probe in &probes {
        assert!(
            probe.contains(HEALTH_PROBE_ID_PREFIX),
            "probe ID must carry private health prefix: {probe}"
        );
    }

    // Verify probe completions counter increased
    assert!(
        probe_state.lock().unwrap().valid_completions >= 2,
        "valid completions recorded"
    );

    // Editor sends normal ping request
    let ping_req = json!({
        "jsonrpc": "2.0",
        "id": 100,
        "method": "ping",
        "params": {}
    });
    session
        .editor
        .as_mut()
        .unwrap()
        .send(WireMessage::LspPayload(ping_req.to_string()))
        .await
        .expect("send ping");

    // Editor receives pong response; it MUST NOT receive probe replies!
    let editor_msg = tokio::time::timeout(WAIT, session.editor.as_mut().unwrap().next())
        .await
        .expect("editor recv")
        .expect("stream")
        .expect("msg");

    match editor_msg {
        WireMessage::LspPayload(raw) => {
            let val: Value = serde_json::from_str(&raw).expect("parse payload");
            assert_eq!(val["id"], 100, "editor only received its own response");
            assert_eq!(val["result"], "pong");
        }
        other => panic!("unexpected wire message: {other:?}"),
    }

    // Check that no leaked probe responses follow
    let extra = tokio::time::timeout(Duration::from_millis(80), session.editor.as_mut().unwrap().next()).await;
    assert!(extra.is_err(), "no probe messages leak to editor stream");

    session.finish().await.expect("clean finish");
    wait_for_exit(pid).await.expect("child exited");
}

#[tokio::test]
async fn editor_ordinary_traffic_defers_health_probes_and_resets_timeouts() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    let pid_file = root.join("server.pid");
    let log_file = root.join("server.log");

    let probe_state = Arc::new(std::sync::Mutex::new(ProbeState::default()));
    let command = python_command(
        root,
        &pid_file,
        &log_file,
        false,
        ReadySignal::default(),
    );
    let servers = Arc::new(EditorServers::default());
    let options = EditorProxyOptions {
        write_budget: SHORT_WRITE,
        teardown_budget: SHORT_TEARDOWN,
        health_probe_interval: Some(Duration::from_millis(80)),
        health_response_timeout: Duration::from_millis(50),
        probe_state: Some(Arc::clone(&probe_state)),
    };

    let mut session = start_session_with_options(root, command, servers, options).await;
    let pid = wait_for_pid(&pid_file).await.expect("pid");

    // Send traffic every 25ms for 250ms (faster than 80ms interval)
    let start = tokio::time::Instant::now();
    let mut id = 1;
    while start.elapsed() < Duration::from_millis(250) {
        let ping_req = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "ping",
            "params": {}
        });
        session
            .editor
            .as_mut()
            .unwrap()
            .send(WireMessage::LspPayload(ping_req.to_string()))
            .await
            .expect("send ping");

        let msg = tokio::time::timeout(WAIT, session.editor.as_mut().unwrap().next())
            .await
            .expect("recv")
            .expect("stream")
            .expect("msg");
        if let WireMessage::LspPayload(raw) = msg {
            let val: Value = serde_json::from_str(&raw).unwrap();
            assert_eq!(val["id"], id);
        }
        id += 1;
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    // Since ordinary traffic was active continuously, 0 health probes should have fired
    assert_eq!(
        probe_state.lock().unwrap().valid_completions,
        0,
        "traffic deferred probes"
    );
    if let Ok(text) = std::fs::read_to_string(&log_file) {
        let probe_count = text.lines().filter(|l| l.starts_with(HEALTH_PROBE_METHOD)).count();
        assert_eq!(probe_count, 0, "no probe requests logged during continuous traffic");
    }

    session.finish().await.expect("clean finish");
    wait_for_exit(pid).await.expect("child exited");
}

#[tokio::test]
async fn loaded_project_stress_defers_health_probes_during_indexing_and_in_flight_requests() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    let pid_file = root.join("server.pid");
    let log_file = root.join("server.log");

    let probe_state = Arc::new(std::sync::Mutex::new(ProbeState::default()));
    let command = python_command(
        root,
        &pid_file,
        &log_file,
        false,
        ReadySignal::Progress,
    );
    let servers = Arc::new(EditorServers::default());
    let options = EditorProxyOptions {
        write_budget: SHORT_WRITE,
        teardown_budget: SHORT_TEARDOWN,
        health_probe_interval: Some(Duration::from_millis(40)),
        health_response_timeout: Duration::from_millis(50),
        probe_state: Some(Arc::clone(&probe_state)),
    };

    let mut session = start_session_with_options(root, command, servers, options).await;
    let pid = wait_for_pid(&pid_file).await.expect("pid");

    // Phase 1: Server begins indexing progress (simulating heavy workspace loading under stress)
    let start_prog = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "startProgress",
        "params": {"token": "idx_token"}
    });
    session
        .editor
        .as_mut()
        .unwrap()
        .send(WireMessage::LspPayload(start_prog.to_string()))
        .await
        .expect("start progress");

    // Drain the response and progress notifications on editor socket
    let mut got_start_resp = false;
    while !got_start_resp {
        if let Some(Ok(WireMessage::LspPayload(raw))) = session.editor.as_mut().unwrap().next().await {
            let val: Value = serde_json::from_str(&raw).unwrap();
            if val.get("id") == Some(&json!(1)) {
                got_start_resp = true;
            }
        }
    }

    // Now session is idle for 120ms (3x probe interval), but server is busy indexing
    tokio::time::sleep(Duration::from_millis(120)).await;
    assert_eq!(
        probe_state.lock().unwrap().valid_completions,
        0,
        "probes deferred while server is busy indexing"
    );

    // End progress
    let end_prog = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "endProgress",
        "params": {"token": "idx_token"}
    });
    session
        .editor
        .as_mut()
        .unwrap()
        .send(WireMessage::LspPayload(end_prog.to_string()))
        .await
        .expect("end progress");

    let mut got_end_resp = false;
    while !got_end_resp {
        if let Some(Ok(WireMessage::LspPayload(raw))) = session.editor.as_mut().unwrap().next().await {
            let val: Value = serde_json::from_str(&raw).unwrap();
            if val.get("id") == Some(&json!(2)) {
                got_end_resp = true;
            }
        }
    }

    // Now progress has ended; probes should resume!
    let probes = wait_for_log_count(&log_file, HEALTH_PROBE_METHOD, 1)
        .await
        .expect("probes after indexing ended");
    assert!(!probes.is_empty(), "probes resume after indexing completes");

    // Phase 2: In-flight request stress test
    // Editor issues slowWork request with 120ms delay
    let slow_req = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "slowWork",
        "params": {"delay_ms": 120}
    });
    let completions_before = probe_state.lock().unwrap().valid_completions;
    session
        .editor
        .as_mut()
        .unwrap()
        .send(WireMessage::LspPayload(slow_req.to_string()))
        .await
        .expect("send slow request");

    // Wait for the slowWork response
    let resp = tokio::time::timeout(WAIT, session.editor.as_mut().unwrap().next())
        .await
        .expect("recv")
        .expect("stream")
        .expect("msg");
    if let WireMessage::LspPayload(raw) = resp {
        let val: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(val["id"], 3);
        assert_eq!(val["result"], "done");
    }

    // While slowWork was in-flight, no additional probes were issued
    assert_eq!(
        probe_state.lock().unwrap().valid_completions,
        completions_before,
        "probes deferred while request was in-flight"
    );

    session.finish().await.expect("clean finish");
    wait_for_exit(pid).await.expect("child exited");
}

#[tokio::test]
async fn three_consecutive_idle_probe_timeouts_retires_the_editor_session_and_process_group() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    let pid_file = root.join("server.pid");
    let log_file = root.join("server.log");

    let probe_state = Arc::new(std::sync::Mutex::new(ProbeState::default()));
    // IGNORE_PROBES = true -> server drops health probes without answering
    let command = python_command(
        root,
        &pid_file,
        &log_file,
        true,
        ReadySignal::default(),
    );
    let servers = Arc::new(EditorServers::default());
    let options = EditorProxyOptions {
        write_budget: SHORT_WRITE,
        teardown_budget: SHORT_TEARDOWN,
        health_probe_interval: Some(Duration::from_millis(30)),
        health_response_timeout: Duration::from_millis(30),
        probe_state: Some(Arc::clone(&probe_state)),
    };

    let session = start_session_with_options(root, command, servers, options).await;
    let pid = wait_for_pid(&pid_file).await.expect("pid");

    // Session task should terminate on its own after 3 consecutive probe timeouts
    let outcome = session.finish().await;
    assert!(outcome.is_ok(), "session retired cleanly after 3 probe timeouts");

    // The owned process group was retired
    wait_for_exit(pid).await.expect("process group reaped");
}

#[tokio::test]
async fn client_request_with_reserved_probe_id_is_rejected() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    let pid_file = root.join("server.pid");
    let log_file = root.join("server.log");

    let command = python_command(
        root,
        &pid_file,
        &log_file,
        false,
        ReadySignal::default(),
    );
    let servers = Arc::new(EditorServers::default());
    let options = EditorProxyOptions {
        write_budget: SHORT_WRITE,
        teardown_budget: SHORT_TEARDOWN,
        health_probe_interval: Some(Duration::from_secs(60)),
        health_response_timeout: Duration::from_secs(5),
        probe_state: None,
    };

    let mut session = start_session_with_options(root, command, servers, options).await;
    let pid = wait_for_pid(&pid_file).await.expect("pid");

    // Client attempts to spoof a health probe ID
    let spoof_id = format!("{HEALTH_PROBE_ID_PREFIX}999:1");
    let spoof_req = json!({
        "jsonrpc": "2.0",
        "id": &spoof_id,
        "method": "ping",
        "params": {}
    });

    session
        .editor
        .as_mut()
        .unwrap()
        .send(WireMessage::LspPayload(spoof_req.to_string()))
        .await
        .expect("send spoof");

    let resp = tokio::time::timeout(WAIT, session.editor.as_mut().unwrap().next())
        .await
        .expect("recv")
        .expect("stream")
        .expect("msg");

    if let WireMessage::LspPayload(raw) = resp {
        let val: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(val["id"], spoof_id);
        assert_eq!(val["error"]["code"], -32600);
        assert!(
            val["error"]["message"].as_str().unwrap().contains("reserved"),
            "error message explains reserved ID"
        );
    } else {
        panic!("expected LspPayload");
    }

    // Verify spoof request never reached the server
    if let Ok(text) = std::fs::read_to_string(&log_file) {
        assert!(
            !text.contains(&spoof_id),
            "spoofed request was filtered and never forwarded to server"
        );
    }

    session.finish().await.expect("clean finish");
    wait_for_exit(pid).await.expect("child exited");
}

#[tokio::test]
async fn real_gopls_editor_session_answers_probes_and_survives_idle_supervision() {
    let Some(gopls) = server_command("go") else {
        eprintln!("skipping real_gopls test: gopls not found");
        return;
    };

    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    let mod_file = root.join("go.mod");
    let src_file = root.join("main.go");

    std::fs::write(&mod_file, "module example.com/probe\n\ngo 1.22\n").expect("write go.mod");
    std::fs::write(
        &src_file,
        "package main\n\nconst Greeting = \"hello\"\n\nfunc main() {\n    println(Greeting)\n}\n",
    )
    .expect("write main.go");

    let probe_state = Arc::new(std::sync::Mutex::new(ProbeState::default()));
    let servers = Arc::new(EditorServers::default());
    let options = EditorProxyOptions {
        write_budget: Duration::from_secs(5),
        teardown_budget: Duration::from_secs(3),
        health_probe_interval: Some(Duration::from_millis(50)),
        health_response_timeout: Duration::from_secs(3),
        probe_state: Some(Arc::clone(&probe_state)),
    };

    let mut session = start_session_with_options(root, gopls, servers, options).await;

    // Send initialize
    let init_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "rootUri": format!("file://{}", root.display()),
            "capabilities": {}
        }
    });
    session
        .editor
        .as_mut()
        .unwrap()
        .send(WireMessage::LspPayload(init_req.to_string()))
        .await
        .expect("send initialize");

    let mut initialized_ok = false;
    let start = tokio::time::Instant::now();
    while start.elapsed() < WAIT {
        if let Ok(Some(Ok(WireMessage::LspPayload(raw)))) =
            tokio::time::timeout(Duration::from_millis(500), session.editor.as_mut().unwrap().next()).await
        {
            if let Ok(val) = serde_json::from_str::<Value>(&raw) {
                if val.get("id") == Some(&json!(1)) && val.get("result").is_some() {
                    initialized_ok = true;
                    break;
                }
            }
        }
    }
    assert!(initialized_ok, "gopls answered initialize");

    // Send initialized notification
    let note = json!({
        "jsonrpc": "2.0",
        "method": "initialized",
        "params": {}
    });
    session
        .editor
        .as_mut()
        .unwrap()
        .send(WireMessage::LspPayload(note.to_string()))
        .await
        .expect("send initialized");

    // Let session idle for 180ms across multiple probe intervals
    tokio::time::sleep(Duration::from_millis(180)).await;

    // Verify gopls answered probes (or method not found errors) and probe completions >= 2
    let completions = probe_state.lock().unwrap().valid_completions;
    assert!(
        completions >= 2,
        "gopls answered at least 2 health probes during idle period; saw {completions}"
    );

    // Send hover request after idle probes to prove session remains fully functional
    let hover_req = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "textDocument/hover",
        "params": {
            "textDocument": { "uri": format!("file://{}", src_file.display()) },
            "position": { "line": 2, "character": 8 }
        }
    });
    session
        .editor
        .as_mut()
        .unwrap()
        .send(WireMessage::LspPayload(hover_req.to_string()))
        .await
        .expect("send hover");

    let mut hover_ok = false;
    let start = tokio::time::Instant::now();
    while start.elapsed() < WAIT {
        if let Ok(Some(Ok(WireMessage::LspPayload(raw)))) =
            tokio::time::timeout(Duration::from_millis(500), session.editor.as_mut().unwrap().next()).await
        {
            if let Ok(val) = serde_json::from_str::<Value>(&raw) {
                if val.get("id") == Some(&json!(2)) {
                    hover_ok = true;
                    break;
                }
            }
        }
    }
    assert!(hover_ok, "gopls answered hover after idle health probes");

    session.finish().await.expect("clean finish");
}
