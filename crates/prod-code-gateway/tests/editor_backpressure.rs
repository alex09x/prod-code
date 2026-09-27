#![cfg(unix)]

use futures_util::{SinkExt, StreamExt};
use prod_code_gateway::editor_proxy::{
    EditorServers, ServerCommand, run_with_budgets, server_command,
};
use prod_code_gateway::workspace::WatchedChange;
use prod_code_protocol::{PathTranslator, ProdCodeCodec, WireMessage};
use serde_json::{Value, json};
use std::future::pending;
use std::path::{Path, PathBuf};
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

fn python_command(root: &Path, name: &str, source: &str, pid_file: &Path) -> ServerCommand {
    let script = root.join(name);
    std::fs::write(&script, source).expect("write fake editor server");
    ServerCommand {
        program: "python3".to_string(),
        args: vec![script.to_string_lossy().into_owned()],
        env: vec![(
            "EDITOR_PID_FILE".to_string(),
            pid_file.to_string_lossy().into_owned(),
        )],
    }
}

async fn start_session(
    root: &Path,
    command: ServerCommand,
    servers: Arc<EditorServers>,
    write_budget: Duration,
    teardown_budget: Duration,
) -> Session {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind proxy");
    let address = listener.local_addr().expect("proxy address");
    let session_root = root.to_path_buf();
    let session_servers = Arc::clone(&servers);
    let task = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.expect("accept editor");
        run_with_budgets(
            Framed::new(socket, ProdCodeCodec::new()),
            PathTranslator::new("/client", &session_root.to_string_lossy()),
            command,
            &session_root,
            session_servers.as_ref(),
            574,
            write_budget,
            teardown_budget,
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

async fn wait_for_registration(servers: &EditorServers, count: usize) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        if servers.count() == count {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "expected {count} editor registrations, found {}",
                servers.count()
            ));
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn wait_for_pids(path: &Path, count: usize) -> Result<Vec<i32>, String> {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        if let Ok(text) = std::fs::read_to_string(path) {
            let pids: Vec<_> = text
                .split_whitespace()
                .filter_map(|value| value.parse().ok())
                .collect();
            if pids.len() == count {
                return Ok(pids);
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!("fake server did not publish {count} owned PIDs"));
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
            return Err(format!("owned editor process {pid} survived cleanup"));
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn process_exists(pid: i32) -> bool {
    (unsafe { libc::kill(pid, 0) }) == 0
        || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

async fn exits_without_forced_cleanup(pid: i32, budget: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        if !process_exists(pid) {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

struct ExactProcessGroup {
    group: i32,
    active: bool,
}

impl ExactProcessGroup {
    fn new(group: i32) -> Self {
        Self {
            group,
            active: true,
        }
    }

    fn retire(&mut self) -> Result<(), String> {
        if !self.active {
            return Ok(());
        }
        let signal = unsafe { libc::kill(-self.group, libc::SIGKILL) };
        if signal == -1 && std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH) {
            return Err(format!(
                "could not retire exact test process group {}: {}",
                self.group,
                std::io::Error::last_os_error()
            ));
        }
        self.active = false;
        Ok(())
    }
}

impl Drop for ExactProcessGroup {
    fn drop(&mut self) {
        if self.active {
            let _ = unsafe { libc::kill(-self.group, libc::SIGKILL) };
        }
    }
}

async fn send_before(
    editor: &mut Framed<TcpStream, ProdCodeCodec>,
    message: WireMessage,
    deadline: tokio::time::Instant,
    action: &str,
) -> Result<(), String> {
    tokio::time::timeout_at(deadline, editor.send(message))
        .await
        .map_err(|_| format!("timed out while {action}"))?
        .map_err(|error| format!("{action}: {error}"))
}

async fn receive_payload_before(
    editor: &mut Framed<TcpStream, ProdCodeCodec>,
    deadline: tokio::time::Instant,
) -> Result<Value, String> {
    loop {
        let message = tokio::time::timeout_at(deadline, editor.next())
            .await
            .map_err(|_| "editor payload deadline elapsed".to_string())?
            .ok_or_else(|| "editor connection closed before its payload".to_string())?
            .map_err(|error| format!("editor protocol error: {error}"))?;
        if let WireMessage::LspPayload(raw) = message {
            return serde_json::from_str(&raw).map_err(|error| format!("invalid JSON: {error}"));
        }
    }
}

async fn receive_payload(editor: &mut Framed<TcpStream, ProdCodeCodec>) -> Result<Value, String> {
    receive_payload_before(editor, tokio::time::Instant::now() + WAIT).await
}

const INITIALIZE_THEN_STALL: &str = r#"
import json, os, sys, time
open(os.environ['EDITOR_PID_FILE'], 'w').write(str(os.getpid()))
def read_frame():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        if line in (b'\r\n', b'\n'):
            break
        if line.lower().startswith(b'content-length:'):
            length = int(line.split(b':', 1)[1])
    return sys.stdin.buffer.read(length)
request = json.loads(read_frame())
body = json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'capabilities':{}}}).encode()
sys.stdout.buffer.write(b'Content-Length: ' + str(len(body)).encode() + b'\r\n\r\n' + body)
sys.stdout.buffer.flush()
time.sleep(60)
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stalled_server_input_cannot_hold_disconnect_or_owned_process() {
    let temp = tempfile::tempdir().expect("temporary editor root");
    let pid_file = temp.path().join("server.pid");
    let command = python_command(
        temp.path(),
        "stall_input.py",
        INITIALIZE_THEN_STALL,
        &pid_file,
    );
    let servers = Arc::new(EditorServers::default());
    let mut session = start_session(
        temp.path(),
        command,
        Arc::clone(&servers),
        SHORT_WRITE,
        SHORT_TEARDOWN,
    )
    .await;
    let editor = session.editor.as_mut().expect("editor connection");
    editor
        .send(WireMessage::LspPayload(
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}).to_string(),
        ))
        .await
        .expect("send initialize");
    assert_eq!(receive_payload(editor).await.unwrap()["id"], 1);
    let pid = wait_for_pids(&pid_file, 1).await.unwrap()[0];

    let large = "x".repeat(256 * 1024);
    for id in 2..20 {
        if editor
            .send(WireMessage::LspPayload(
                json!({"jsonrpc":"2.0","id":id,"method":"unknown/large","params":{"value":large}})
                    .to_string(),
            ))
            .await
            .is_err()
        {
            break;
        }
    }
    let _ = editor
        .send(WireMessage::Disconnect {
            reason: "input stalled".to_string(),
        })
        .await;
    session
        .finish()
        .await
        .expect("bounded stalled-input cleanup");
    wait_for_exit(pid).await.unwrap();
    assert_eq!(servers.count(), 0);
}

const FLOOD_OUTPUT: &str = r#"
import json, os, sys, time
open(os.environ['EDITOR_PID_FILE'], 'w').write(str(os.getpid()))
value = 'x' * (32 * 1024)
for number in range(4096):
    body = json.dumps({'jsonrpc':'2.0','method':'window/logMessage','params':{'type':3,'message':value}}).encode()
    sys.stdout.buffer.write(b'Content-Length: ' + str(len(body)).encode() + b'\r\n\r\n' + body)
    sys.stdout.buffer.flush()
time.sleep(60)
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_nonreading_editor_cannot_block_disconnect_and_retirement() {
    let temp = tempfile::tempdir().expect("temporary editor root");
    let pid_file = temp.path().join("server.pid");
    let command = python_command(temp.path(), "flood.py", FLOOD_OUTPUT, &pid_file);
    let servers = Arc::new(EditorServers::default());
    let mut session = start_session(
        temp.path(),
        command,
        Arc::clone(&servers),
        SHORT_WRITE,
        SHORT_TEARDOWN,
    )
    .await;
    wait_for_registration(&servers, 1).await.unwrap();
    let pid = wait_for_pids(&pid_file, 1).await.unwrap()[0];
    tokio::time::sleep(Duration::from_millis(100)).await;
    session
        .editor
        .as_mut()
        .unwrap()
        .send(WireMessage::Disconnect {
            reason: "not reading output".to_string(),
        })
        .await
        .expect("disconnect still reaches the proxy");
    session.finish().await.expect("bounded output cleanup");
    wait_for_exit(pid).await.unwrap();
    assert_eq!(servers.count(), 0);
}

const PROCESS_TREE: &str = r#"
import os, signal, time
signal.signal(signal.SIGTERM, signal.SIG_IGN)
child = os.fork()
if child == 0:
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    while True: time.sleep(1)
open(os.environ['EDITOR_PID_FILE'], 'w').write(str(os.getpid()) + ' ' + str(child))
while True: time.sleep(1)
"#;

const EXITING_PARENT_TREE: &str = r#"
import os, signal, time
child = os.fork()
if child == 0:
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    while True: time.sleep(1)
open(os.environ['EDITOR_PID_FILE'], 'w').write(str(os.getpid()) + ' ' + str(child))
while not os.path.exists(os.environ['EDITOR_RELEASE_FILE']): time.sleep(0.01)
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_normally_exited_parent_still_retires_its_owned_descendants() {
    let temp = tempfile::tempdir().expect("temporary editor root");
    let pid_file = temp.path().join("tree.pid");
    let release_file = temp.path().join("release-parent");
    let mut command = python_command(
        temp.path(),
        "exiting_parent.py",
        EXITING_PARENT_TREE,
        &pid_file,
    );
    command.env.push((
        "EDITOR_RELEASE_FILE".to_string(),
        release_file.to_string_lossy().into_owned(),
    ));
    let servers = Arc::new(EditorServers::default());
    let session = start_session(
        temp.path(),
        command,
        Arc::clone(&servers),
        SHORT_WRITE,
        SHORT_TEARDOWN,
    )
    .await;
    let pids = wait_for_pids(&pid_file, 2).await.unwrap();
    let mut cleanup = ExactProcessGroup::new(pids[0]);
    std::fs::write(&release_file, b"exit").expect("release fake parent");

    let outcome = session.finish().await;
    let descendant_retired = exits_without_forced_cleanup(pids[1], SHORT_WRITE).await;
    cleanup.retire().expect("retire exact test process group");
    wait_for_exit(pids[1]).await.unwrap();

    outcome.expect("normally exited parent session cleanup");
    assert!(
        descendant_retired,
        "owned descendant {} survived after leader {} was reaped",
        pids[1], pids[0]
    );
    assert_eq!(servers.count(), 0);
}

const MALFORMED_TREE: &str = r#"
import os, signal, sys, time
child = os.fork()
if child == 0:
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    while True: time.sleep(1)
open(os.environ['EDITOR_PID_FILE'], 'w').write(str(os.getpid()) + ' ' + str(child))
while not os.path.exists(os.environ['EDITOR_RELEASE_FILE']): time.sleep(0.01)
sys.stdout.buffer.write(b'Content-Length: 2\r\nContent-Length: 2\r\n\r\n{}')
sys.stdout.buffer.flush()
while True: time.sleep(1)
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_early_reader_error_retires_the_exact_owned_process_group() {
    let temp = tempfile::tempdir().expect("temporary editor root");
    let pid_file = temp.path().join("tree.pid");
    let release_file = temp.path().join("release-malformed-frame");
    let mut command = python_command(temp.path(), "malformed_tree.py", MALFORMED_TREE, &pid_file);
    command.env.push((
        "EDITOR_RELEASE_FILE".to_string(),
        release_file.to_string_lossy().into_owned(),
    ));
    let servers = Arc::new(EditorServers::default());
    let session = start_session(
        temp.path(),
        command,
        Arc::clone(&servers),
        SHORT_WRITE,
        SHORT_TEARDOWN,
    )
    .await;
    let pids = wait_for_pids(&pid_file, 2).await.unwrap();
    let mut cleanup = ExactProcessGroup::new(pids[0]);
    std::fs::write(&release_file, b"malformed").expect("release malformed frame");

    let outcome = session.finish().await;
    let mut failures = Vec::new();
    for pid in &pids {
        if let Err(error) = wait_for_exit(*pid).await {
            failures.push(error);
        }
    }
    cleanup.retire().expect("retire exact test process group");

    if let Err(error) = outcome {
        failures.push(error);
    }
    assert!(failures.is_empty(), "{}", failures.join("; "));
    assert_eq!(servers.count(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_a_session_helper_does_not_detach_its_owned_task() {
    struct Completion(Option<tokio::sync::oneshot::Sender<()>>);
    impl Drop for Completion {
        fn drop(&mut self) {
            if let Some(done) = self.0.take() {
                let _ = done.send(());
            }
        }
    }

    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (done_tx, mut done_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let _completion = Completion(Some(done_tx));
        let _ = started_tx.send(());
        pending::<()>().await;
        Ok(())
    });
    struct AbortOnDrop(tokio::task::AbortHandle);
    impl Drop for AbortOnDrop {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let forced_cleanup = task.abort_handle();
    let _task_abort = AbortOnDrop(forced_cleanup.clone());
    let session = Session {
        editor: None,
        task: Some(task),
    };
    tokio::time::timeout(WAIT, started_rx)
        .await
        .expect("owned task readiness is bounded")
        .expect("owned task started");
    let helper = tokio::spawn(session.finish());
    let _helper_abort = AbortOnDrop(helper.abort_handle());
    tokio::time::sleep(Duration::from_millis(50)).await;
    helper.abort();
    let helper_result = helper.await;
    let terminated_by_owner = tokio::time::timeout(SHORT_WRITE, &mut done_rx)
        .await
        .is_ok();
    if !terminated_by_owner {
        forced_cleanup.abort();
        tokio::time::timeout(WAIT, &mut done_rx)
            .await
            .expect("forced cleanup terminates exact owned task")
            .expect("owned task reports completion");
    }

    assert!(
        helper_result
            .as_ref()
            .is_err_and(|error| error.is_cancelled()),
        "helper itself was cancelled: {helper_result:?}"
    );
    assert!(
        terminated_by_owner,
        "cancelling Session::finish detached its owned task"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_before_readiness_retires_the_owned_group_and_registration() {
    let temp = tempfile::tempdir().expect("temporary editor root");
    let pid_file = temp.path().join("tree.pid");
    let command = python_command(temp.path(), "tree.py", PROCESS_TREE, &pid_file);
    let servers = Arc::new(EditorServers::default());
    let session = start_session(
        temp.path(),
        command,
        Arc::clone(&servers),
        SHORT_WRITE,
        SHORT_TEARDOWN,
    )
    .await;
    wait_for_registration(&servers, 1).await.unwrap();
    let pids = wait_for_pids(&pid_file, 2).await.unwrap();
    session.cancel().await.expect("session cancellation");
    for pid in pids {
        wait_for_exit(pid).await.unwrap();
    }
    assert_eq!(servers.count(), 0);
}

const FINAL_FRAME: &str = r#"
import json, os, sys
open(os.environ['EDITOR_PID_FILE'], 'w').write(str(os.getpid()))
body = json.dumps({'jsonrpc':'2.0','id':9,'result':{'last':True}}).encode()
sys.stdout.buffer.write(b'Content-Length: ' + str(len(body)).encode() + b'\r\n\r\n' + body)
sys.stdout.buffer.flush()
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_finite_final_response_is_delivered_before_eof_and_cleanup() {
    let temp = tempfile::tempdir().expect("temporary editor root");
    let pid_file = temp.path().join("server.pid");
    let command = python_command(temp.path(), "final.py", FINAL_FRAME, &pid_file);
    let servers = Arc::new(EditorServers::default());
    let mut session = start_session(
        temp.path(),
        command,
        Arc::clone(&servers),
        SHORT_WRITE,
        SHORT_TEARDOWN,
    )
    .await;
    let pid = wait_for_pids(&pid_file, 1).await.unwrap()[0];
    let final_message = receive_payload(session.editor.as_mut().unwrap())
        .await
        .unwrap();
    assert_eq!(
        final_message,
        json!({"jsonrpc":"2.0","id":9,"result":{"last":true}})
    );
    let eof = tokio::time::timeout(WAIT, session.editor.as_mut().unwrap().next())
        .await
        .expect("connection closes after final response");
    assert!(
        eof.is_none(),
        "no frame follows the finite final response: {eof:?}"
    );
    session.finish().await.expect("finite session cleanup");
    wait_for_exit(pid).await.unwrap();
    assert_eq!(servers.count(), 0);
}

async fn lsp_request(
    editor: &mut Framed<TcpStream, ProdCodeCodec>,
    id: u64,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    lsp_request_before(
        editor,
        id,
        method,
        params,
        tokio::time::Instant::now() + WAIT,
    )
    .await
}

async fn lsp_request_before(
    editor: &mut Framed<TcpStream, ProdCodeCodec>,
    id: u64,
    method: &str,
    params: Value,
    deadline: tokio::time::Instant,
) -> Result<Value, String> {
    send_before(
        editor,
        WireMessage::LspPayload(
            json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}).to_string(),
        ),
        deadline,
        &format!("sending {method}"),
    )
    .await?;
    loop {
        let value = receive_payload_before(editor, deadline).await?;
        if let (Some(request_id), Some(request_method)) = (
            value.get("id").cloned(),
            value.get("method").and_then(Value::as_str),
        ) {
            let result = if request_method == "workspace/configuration" {
                json!([])
            } else {
                Value::Null
            };
            send_before(
                editor,
                WireMessage::LspPayload(
                    json!({"jsonrpc":"2.0","id":request_id,"result":result}).to_string(),
                ),
                deadline,
                &format!("answering {request_method}"),
            )
            .await?;
            continue;
        }
        if value.get("id").and_then(Value::as_u64) == Some(id) {
            return Ok(value);
        }
    }
}

const CONTINUOUS_NOTIFICATIONS: &str = r#"
import json, os, sys
open(os.environ['EDITOR_PID_FILE'], 'w').write(str(os.getpid()))
def read_frame():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        if line in (b'\r\n', b'\n'):
            break
        if line.lower().startswith(b'content-length:'):
            length = int(line.split(b':', 1)[1])
    return sys.stdin.buffer.read(length)
read_frame()
number = 0
while True:
    body = json.dumps({'jsonrpc':'2.0','method':'window/logMessage','params':{'type':3,'message':str(number)}}).encode()
    sys.stdout.buffer.write(b'Content-Length: ' + str(len(body)).encode() + b'\r\n\r\n' + body)
    sys.stdout.buffer.flush()
    number += 1
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unrelated_notifications_cannot_extend_an_lsp_request_deadline() {
    let temp = tempfile::tempdir().expect("temporary editor root");
    let pid_file = temp.path().join("server.pid");
    let command = python_command(
        temp.path(),
        "continuous_notifications.py",
        CONTINUOUS_NOTIFICATIONS,
        &pid_file,
    );
    let servers = Arc::new(EditorServers::default());
    let mut session = start_session(
        temp.path(),
        command,
        Arc::clone(&servers),
        SHORT_WRITE,
        SHORT_TEARDOWN,
    )
    .await;
    let pid = wait_for_pids(&pid_file, 1).await.unwrap()[0];
    let started = tokio::time::Instant::now();
    let result = lsp_request_before(
        session.editor.as_mut().unwrap(),
        44,
        "test/noReply",
        json!({}),
        started + SHORT_WRITE,
    )
    .await;
    let elapsed = started.elapsed();
    session.finish().await.expect("notification flood cleanup");
    wait_for_exit(pid).await.unwrap();

    assert!(
        result
            .as_ref()
            .is_err_and(|error| error.contains("deadline")),
        "request must end at its absolute deadline: {result:?}"
    );
    assert!(
        elapsed < WAIT,
        "request exceeded its bounded proof: {elapsed:?}"
    );
    assert_eq!(servers.count(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_gopls_keeps_initialize_open_hover_watch_and_disconnect_intact() {
    let temp = tempfile::tempdir().expect("temporary Go root");
    let root = temp.path().to_path_buf();
    let source = root.join("main.go");
    std::fs::write(
        root.join("go.mod"),
        "module example.com/editorproof\n\ngo 1.22\n",
    )
    .expect("write go.mod");
    let text = "package main\n\nconst Greeting = \"hello\"\n\nfunc main() { println(Greeting) }\n";
    std::fs::write(&source, text).expect("write Go source");
    let gopls = server_command("go").expect("the required build node has gopls");
    let pid_file = root.join("gopls.pid");
    let command = ServerCommand {
        program: "/bin/sh".to_string(),
        args: vec![
            "-c".to_string(),
            "printf '%s\\n' \"$$\" > \"$EDITOR_PID_FILE\"; exec \"$GOPLS\"".to_string(),
        ],
        env: vec![
            (
                "EDITOR_PID_FILE".to_string(),
                pid_file.to_string_lossy().into_owned(),
            ),
            ("GOPLS".to_string(), gopls.program),
        ],
    };
    let servers = Arc::new(EditorServers::default());
    let mut session = start_session(
        &root,
        command,
        Arc::clone(&servers),
        Duration::from_secs(10),
        Duration::from_secs(2),
    )
    .await;
    let pid = wait_for_pids(&pid_file, 1).await.unwrap()[0];
    let editor = session.editor.as_mut().unwrap();
    let root_uri = format!("file://{}", root.display());
    let source_uri = format!("file://{}", source.display());
    let initialized = lsp_request(
        editor,
        1,
        "initialize",
        json!({
            "processId": std::process::id(),
            "rootUri": root_uri,
            "capabilities": {"workspace":{"configuration":true,"didChangeWatchedFiles":{"dynamicRegistration":true}}},
            "initializationOptions": {}
        }),
    )
    .await
    .unwrap();
    assert!(initialized["result"]["capabilities"].is_object());
    send_before(
        editor,
        WireMessage::LspPayload(
            json!({"jsonrpc":"2.0","method":"initialized","params":{}}).to_string(),
        ),
        tokio::time::Instant::now() + WAIT,
        "sending initialized",
    )
    .await
    .unwrap();
    send_before(
        editor,
        WireMessage::LspPayload(
            json!({
                "jsonrpc":"2.0",
                "method":"textDocument/didOpen",
                "params":{"textDocument":{"uri":source_uri,"languageId":"go","version":1,"text":text}}
            })
            .to_string(),
        ),
        tokio::time::Instant::now() + WAIT,
        "sending didOpen",
    )
    .await
    .unwrap();
    let hover = lsp_request(
        editor,
        2,
        "textDocument/hover",
        json!({"textDocument":{"uri":source_uri},"position":{"line":4,"character":24}}),
    )
    .await
    .unwrap();
    assert!(
        !hover["result"].is_null(),
        "gopls returns a nonempty hover: {hover}"
    );

    let watched_text = text.replace("hello", "hello after watched change");
    std::fs::write(&source, watched_text).expect("write watched Go source bytes");
    servers
        .notify(&[(PathBuf::from(&source), WatchedChange::Changed)])
        .await;
    let pong_deadline = tokio::time::Instant::now() + WAIT;
    send_before(editor, WireMessage::Ping, pong_deadline, "sending ping")
        .await
        .unwrap();
    loop {
        match tokio::time::timeout_at(pong_deadline, editor.next())
            .await
            .expect("pong deadline elapsed")
        {
            Some(Ok(WireMessage::Pong)) => break,
            Some(Ok(WireMessage::LspPayload(raw))) => {
                let value: Value = serde_json::from_str(&raw).unwrap();
                if let (Some(request_id), Some(_)) = (
                    value.get("id").cloned(),
                    value.get("method").and_then(Value::as_str),
                ) {
                    send_before(
                        editor,
                        WireMessage::LspPayload(
                            json!({"jsonrpc":"2.0","id":request_id,"result":null}).to_string(),
                        ),
                        pong_deadline,
                        "answering gopls before pong",
                    )
                    .await
                    .unwrap();
                }
            }
            other => panic!("gopls session ended before pong: {other:?}"),
        }
    }
    send_before(
        editor,
        WireMessage::Disconnect {
            reason: "real gopls proof complete".to_string(),
        },
        tokio::time::Instant::now() + WAIT,
        "sending disconnect",
    )
    .await
    .unwrap();
    session.finish().await.expect("real gopls cleanup");
    wait_for_exit(pid).await.unwrap();
    assert_eq!(servers.count(), 0);
}

const BARRIER_FLOOD_OUTPUT: &str = r#"
import json, os, sys, time
open(os.environ['EDITOR_PID_FILE'], 'w').write(str(os.getpid()))
value = 'x' * (32 * 1024)
for number in range(4096):
    body = json.dumps({'jsonrpc':'2.0','method':'window/logMessage','params':{'type':3,'message':value}}).encode()
    sys.stdout.buffer.write(b'Content-Length: ' + str(len(body)).encode() + b'\r\n\r\n' + body)
    sys.stdout.buffer.flush()
    if number == 320:
        open(os.environ['EDITOR_FLOOD_READY_FILE'], 'w').write('ready')
time.sleep(60)
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_connected_nonreading_editor_does_not_block_disconnect() {
    let temp = tempfile::tempdir().expect("temporary editor root");
    let pid_file = temp.path().join("server.pid");
    let ready_file = temp.path().join("flood.ready");
    let mut command = python_command(
        temp.path(),
        "barrier_flood.py",
        BARRIER_FLOOD_OUTPUT,
        &pid_file,
    );
    command.env.push((
        "EDITOR_FLOOD_READY_FILE".to_string(),
        ready_file.to_string_lossy().into_owned(),
    ));
    let servers = Arc::new(EditorServers::default());
    let mut session = start_session(
        temp.path(),
        command,
        Arc::clone(&servers),
        SHORT_WRITE,
        SHORT_TEARDOWN,
    )
    .await;
    let pid = wait_for_pids(&pid_file, 1).await.unwrap()[0];
    let readiness = tokio::time::timeout(WAIT, async {
        while !ready_file.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    let send = send_before(
        session.editor.as_mut().unwrap(),
        WireMessage::Disconnect {
            reason: "connected nonreading editor".to_string(),
        },
        tokio::time::Instant::now() + WAIT,
        "sending disconnect without closing editor",
    )
    .await;
    // Keep the socket open and unread through the observation. Closing it first would release
    // a legacy blocked socket writer and turn this regression into a false pass.
    let outcome = tokio::time::timeout(
        Duration::from_secs(2),
        session.task.as_mut().expect("owned session task"),
    )
    .await;
    let finished_without_peer_close = matches!(&outcome, Ok(Ok(Ok(()))));
    if outcome.is_ok() {
        session.task.take();
    } else {
        session.task.as_ref().unwrap().abort();
        let _ = tokio::time::timeout(WAIT, session.task.as_mut().unwrap()).await;
    }
    drop(session);
    let cleanup = wait_for_exit(pid).await;
    assert!(
        readiness.is_ok(),
        "the controlled output flood reached its barrier"
    );
    assert!(send.is_ok(), "disconnect reached the proxy: {send:?}");
    cleanup.expect("the exact owned child is reaped even after a failing baseline observation");
    assert!(
        finished_without_peer_close,
        "disconnect must retire a connected nonreading editor within the absolute budget: {outcome:?}"
    );
    assert_eq!(servers.count(), 0);
}
