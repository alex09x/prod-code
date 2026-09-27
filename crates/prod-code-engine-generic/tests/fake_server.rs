//! The generic LSP adapter, driven against a language server written for the purpose.
//!
//! The adapter supervises somebody else's process: it frames requests, matches answers to
//! them, notices published diagnostics, times out, and decides whether the child is still
//! alive. None of that is about any particular language, and all of it is easier to test
//! against a server that does exactly what a test needs than against clangd or pyright, which
//! do a great deal else and are not installed everywhere.
//!
//! So the server here is a short Python script. It speaks the framing (`Content-Length`, a
//! blank line, the JSON), answers `initialize`, echoes what it is asked, publishes a
//! diagnostic when told to, stays silent when a test wants a timeout, and exits when told to
//! die so that the adapter can notice.

use prod_code_engine_generic::{GenericLspConfig, GenericLspEngine, Unavailable};
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

/// A language server that does only what these tests need.
const SERVER: &str = r#"
import json, os, sys, threading, time

LOCK = threading.Lock()
INITIALIZE_COUNT = 0
if os.environ.get("FAKE_PID_FILE"):
    with open(os.environ["FAKE_PID_FILE"], "w") as pid_file:
        pid_file.write(str(os.getpid()))

def send(message):
    body = json.dumps(message).encode()
    with LOCK:
        sys.stdout.buffer.write(b"Content-Length: %d\r\n" % len(body))
        if os.environ.get("FAKE_CONTENT_TYPE"):
            sys.stdout.buffer.write(b"Content-Type: application/vscode-jsonrpc; charset=utf-8\r\n")
        sys.stdout.buffer.write(b"\r\n")
        sys.stdout.buffer.write(body)
        sys.stdout.buffer.flush()

def publish(uri, version, text):
    publish_items(uri, version, [{
        "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}},
        "severity": 1,
        "message": text
    }])

def publish_items(uri, version, items):
    send({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": {
        "uri": uri, "version": version, "diagnostics": items
    }})

def read():
    length = 0
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.strip()
        if not line:
            break
        if line.lower().startswith(b"content-length:"):
            length = int(line.split(b":")[1])
    if not length:
        return None
    if os.environ.get("FAKE_READING_FILE") and length > 1024 * 1024:
        with open(os.environ["FAKE_READING_FILE"], "w") as reading_file:
            reading_file.write(str(length))
    body = b""
    while len(body) < length:
        chunk = sys.stdin.buffer.read(min(length - len(body), 4096))
        if not chunk:
            return None
        body += chunk
        if os.environ.get("FAKE_SLOW_READ"):
            time.sleep(float(os.environ["FAKE_SLOW_READ"]))
    return json.loads(body)

PULLS = {}
LINES = {}
REQUESTS = []
INDEXED = [not os.environ.get("FAKE_INDEXING")]

def end_indexing():
    INDEXED[0] = True
    send({"jsonrpc": "2.0", "method": "$/progress", "params": {"token": "index", "value": {"kind": "end"}}})

while True:
    message = read()
    if message is None:
        break
    method = message.get("method", "")
    REQUESTS.append(method)
    if os.environ.get("FAKE_SEEN_FILE"):
        with open(os.environ["FAKE_SEEN_FILE"], "a") as seen_file:
            seen_file.write(method + "\n")
    if method == "textDocument/didOpen":
        LINES[message["params"]["textDocument"]["uri"]] = len(message["params"]["textDocument"]["text"].splitlines())
    if method == "initialize":
        INITIALIZE_COUNT += 1
        configured = os.environ.get("FAKE_INITIALIZE_RESPONSE")
        if INITIALIZE_COUNT > 1:
            configured = os.environ.get("FAKE_INITIALIZE_AFTER_FIRST", configured)
            if os.environ.get("FAKE_INITIALIZE_DELAY_AFTER_FIRST"):
                time.sleep(float(os.environ["FAKE_INITIALIZE_DELAY_AFTER_FIRST"]))
        if configured == "silence":
            continue
        if configured:
            response = json.loads(configured)
            response.update({"jsonrpc": "2.0", "id": message["id"]})
            send(response)
        else:
            capabilities = {} if os.environ.get("FAKE_EMPTY_CAPABILITIES") else {"hoverProvider": True, "diagnosticProvider": {"interFileDependencies": False}}
            send({"jsonrpc": "2.0", "id": message["id"], "result": {"capabilities": capabilities}})
    elif method == "initialized" and os.environ.get("FAKE_CLOSE_STDIN"):
        os.close(0)
        threading.Event().wait()
    elif method == "initialized" and os.environ.get("FAKE_HUGE_AUTO_REQUEST"):
        send({"jsonrpc": "2.0", "id": 7001, "method": "x" * (8 * 1024 * 1024), "params": {}})
        threading.Event().wait()
    elif method == "initialized" and os.environ.get("FAKE_STOP_READING"):
        # Keep the process alive but abandon stdin, as a wedged language server can.
        threading.Event().wait()
    elif method == "initialized" and os.environ.get("FAKE_INDEXING"):
        # As clangd does: create a progress token, begin indexing, report, and end it after
        # FAKE_INDEXING seconds (or never), answering workspace/symbol with nothing until then.
        send({"jsonrpc": "2.0", "id": 7000, "method": "window/workDoneProgress/create", "params": {"token": "index"}})
        send({"jsonrpc": "2.0", "method": "$/progress", "params": {"token": "index", "value": {"kind": "begin", "title": "indexing", "percentage": 0}}})
        send({"jsonrpc": "2.0", "method": "$/progress", "params": {"token": "index", "value": {"kind": "report", "message": "1/2", "percentage": 50}}})
        if os.environ["FAKE_INDEXING"] != "forever":
            threading.Timer(float(os.environ["FAKE_INDEXING"]), end_indexing).start()
    elif method == "workspace/symbol":
        found = [{"name": "indexed", "kind": 12, "location": {"uri": "file:///wherever/a.txt",
                  "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 5}}}}]
        send({"jsonrpc": "2.0", "id": message["id"], "result": found if INDEXED[0] else []})
    elif method == "textDocument/hover" and os.environ.get("FAKE_COLLIDE"):
        # A request of the server's own that happens to carry the id of the question it is
        # answering, as gopls's window/workDoneProgress/create did; the answer follows once the
        # client has answered it.
        send({"jsonrpc": "2.0", "id": message["id"], "method": "window/workDoneProgress/create", "params": {"token": "t"}})
        reply = read()
        send({"jsonrpc": "2.0", "id": message["id"], "result": {
            "contents": {"kind": "markdown", "value": "answered after the client answered %s" % json.dumps(reply.get("result"))}
        }})
    elif method == "textDocument/hover":
        send({"jsonrpc": "2.0", "id": message["id"], "result": {
            "contents": {"kind": "markdown", "value": "the fake server answered"}
        }})
    elif method == "textDocument/didOpen" and "silent" in message["params"]["textDocument"]["text"]:
        # A server that has not built the opened text yet, or never will: it publishes nothing.
        pass
    elif method == "textDocument/didOpen" and "late-clean" in message["params"]["textDocument"]["text"]:
        # The build of the opened text finds nothing, and says so, for its version, a moment later.
        document = message["params"]["textDocument"]
        threading.Timer(0.4, publish_items, (document["uri"], document["version"], [])).start()
    elif method == "textDocument/didOpen" and os.environ.get("FAKE_ECHO_CHANGE"):
        # Ownership probes compare exact text. Version their first publication too so an
        # in-flight unversioned open cannot race a subsequent restore notification.
        document = message["params"]["textDocument"]
        publish(document["uri"], document["version"], document["text"])
    elif method == "textDocument/didOpen":
        uri = message["params"]["textDocument"]["uri"]
        send({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": {
            "uri": uri,
            "diagnostics": [{
                "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}},
                "severity": 1,
                "message": "something the fake server disliked"
            }]
        }})
    elif method == "textDocument/diagnostic" and os.environ.get("FAKE_SEMANTIC_AFTER"):
        # As sourcekit-lsp before it has a package's build settings: syntax only, so nothing,
        # until the Nth pull, then the type error on the last line of the opened text.
        uri = message["params"]["textDocument"]["uri"]
        PULLS[uri] = PULLS.get(uri, 0) + 1
        items = []
        if PULLS[uri] >= int(os.environ["FAKE_SEMANTIC_AFTER"]):
            last = LINES.get(uri, 1) - 1
            items = [{"range": {"start": {"line": last, "character": 0}, "end": {"line": last, "character": 1}},
                      "severity": 1, "message": "cannot convert value of type 'String' to specified type 'Int'"}]
        send({"jsonrpc": "2.0", "id": message["id"], "result": {"kind": "full", "items": items}})
    elif method == "textDocument/diagnostic" and os.environ.get("FAKE_RESPONSE"):
        response = json.loads(os.environ["FAKE_RESPONSE"])
        response.update({"jsonrpc": "2.0", "id": message["id"]})
        send(response)
    elif method == "textDocument/diagnostic":
        # A pull, answered as sourcekit-lsp answers it, or refused as clangd refuses it.
        if os.environ.get("FAKE_NO_PULL"):
            send({"jsonrpc": "2.0", "id": message["id"], "error": {"code": -32601, "message": "method not found"}})
        else:
            send({"jsonrpc": "2.0", "id": message["id"], "result": {"kind": "full", "items": [{
                "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}},
                "severity": 1,
                "message": "pulled"
            }]}})
    elif method == "textDocument/didChange":
        # What clangd does: the build of the text before the change is published after the
        # change has arrived, and the changed text's build a moment later.
        uri = message["params"]["textDocument"]["uri"]
        version = message["params"]["textDocument"]["version"]
        publish(uri, version - 1, "from the text before the change")
        # A changed text marked `stale-only` is never built: only the old text's build arrives.
        changed = message["params"]["contentChanges"][-1]["text"]
        if "stale-only" not in changed:
            result = changed if os.environ.get("FAKE_ECHO_CHANGE") else "from the text as changed"
            threading.Timer(0.3, publish, (uri, version, result)).start()
    elif method == "workspace/executeCommand":
        command = message["params"].get("command", "")
        if command == "prodCode/edit":
            # A server that asks the client to apply an edit while the command runs.
            send({"jsonrpc": "2.0", "id": 9001, "method": "workspace/applyEdit", "params": {
                "edit": {"changes": {"file:///wherever/a.txt": [{
                    "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 5}},
                    "newText": "edited"
                }]}}
            }})
            send({"jsonrpc": "2.0", "id": message["id"], "result": None})
        elif command == "prodCode/refuse":
            send({"jsonrpc": "2.0", "id": message["id"], "error": {"code": -32000, "message": "the command was refused"}})
        else:
            send({"jsonrpc": "2.0", "id": message["id"], "result": None})
    elif method == "textDocument/didClose":
        # What clangd does for a document just closed: clears its diagnostics, without a version.
        publish_items(message["params"]["textDocument"]["uri"], None, [])
    elif method == "prodCode/delay":
        threading.Timer(0.4, send, ({"jsonrpc": "2.0", "id": message["id"], "result": "late"},)).start()
    elif method == "prodCode/seen":
        send({"jsonrpc": "2.0", "id": message["id"], "result": REQUESTS})
    elif method == "prodCode/publishTwoVersions":
        uri = message["params"]["uri"]
        publish(uri, message["params"]["first"], message["params"]["firstMessage"])
        publish(uri, message["params"]["second"], message["params"]["secondMessage"])
        send({"jsonrpc": "2.0", "id": message["id"], "result": None})
    elif method == "prodCode/brokenFrame":
        kind = message["params"]["kind"]
        data = {
            "duplicate": b"Content-Length: 2\r\nContent-Length: 2\r\n\r\n{}",
            "oversize": b"Content-Length: 268435457\r\n\r\n",
            "header": b"A" * 65537,
            "truncated": b"Content-Length: 10\r\n\r\n{}",
        }[kind]
        sys.stdout.buffer.write(data)
        sys.stdout.buffer.flush()
        if kind == "truncated":
            sys.exit(0)
        threading.Event().wait()
    elif method == "prodCode/silence":
        # Answer nothing at all, so the caller's timeout is the only way out.
        pass
    elif method == "prodCode/die":
        sys.exit(0)
    elif method == "shutdown":
        send({"jsonrpc": "2.0", "id": message["id"], "result": None})
    elif method == "exit":
        break
"#;

fn config(script: &Path) -> GenericLspConfig {
    GenericLspConfig {
        command: "python3".to_string(),
        args: vec![script.to_string_lossy().into_owned()],
        env: HashMap::new(),
        ..Default::default()
    }
}

/// A request from the server that carries the id of a question in flight is answered as a
/// request, not taken for the question's answer: gopls's `window/workDoneProgress/create` was,
/// which left the question empty and gopls waiting (#391).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_server_request_with_the_id_of_a_question_is_not_its_answer() {
    let (dir, script) = workspace();
    let mut settings = config(&script);
    settings
        .env
        .insert("FAKE_COLLIDE".to_string(), "1".to_string());
    let engine = GenericLspEngine::spawn(dir.path(), settings)
        .await
        .expect("the server starts");
    let answer = engine
        .send_request(
            "textDocument/hover",
            serde_json::json!({ "textDocument": { "uri": "file:///wherever/a.txt" }, "position": { "line": 0, "character": 0 } }),
        )
        .await
        .expect("an answer");
    assert_eq!(
        answer["result"]["contents"]["value"], "answered after the client answered null",
        "{answer}"
    );
}

/// A question answered from the index waits until the server has ended its indexing progress,
/// and gets the whole answer; a question that is not waits for nothing (#391).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_question_from_the_index_waits_until_the_server_has_indexed() {
    let (dir, script) = workspace();
    let mut settings = config(&script);
    settings
        .env
        .insert("FAKE_INDEXING".to_string(), "0.6".to_string());
    settings.ready = prod_code_protocol::readiness::ReadySignal::Progress;
    let engine = GenericLspEngine::spawn(dir.path(), settings)
        .await
        .expect("the server starts");
    assert!(engine.readiness_known());

    let asked = std::time::Instant::now();
    let answer = engine
        .send_request(
            "workspace/symbol",
            serde_json::json!({ "query": "indexed" }),
        )
        .await
        .expect("an answer");
    assert!(
        asked.elapsed() >= Duration::from_millis(400),
        "it waited for the index: {:?}",
        asked.elapsed()
    );
    assert_eq!(
        answer["result"].as_array().map(Vec::len),
        Some(1),
        "{answer}"
    );
    assert!(
        answer
            .get(prod_code_protocol::readiness::BUSY_MEMBER)
            .is_none(),
        "{answer}"
    );
    assert_eq!(engine.busy(), None);
}

/// A server still indexing when the wait ends is asked anyway, and the answer carries how far
/// it got; a hover in the meantime does not wait at all (#391).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_server_still_indexing_when_the_wait_ends_is_asked_with_a_note() {
    let (dir, script) = workspace();
    let mut settings = config(&script);
    settings
        .env
        .insert("FAKE_INDEXING".to_string(), "forever".to_string());
    settings.ready = prod_code_protocol::readiness::ReadySignal::Progress;
    settings.index_wait = Duration::from_millis(300);
    let engine = GenericLspEngine::spawn(dir.path(), settings)
        .await
        .expect("the server starts");

    let asked = std::time::Instant::now();
    let hover = engine
        .send_request(
            "textDocument/hover",
            serde_json::json!({ "textDocument": { "uri": "file:///wherever/a.txt" }, "position": { "line": 0, "character": 0 } }),
        )
        .await
        .expect("a hover");
    assert!(
        asked.elapsed() < Duration::from_millis(250),
        "{:?}",
        asked.elapsed()
    );
    assert!(
        hover
            .get(prod_code_protocol::readiness::BUSY_MEMBER)
            .is_none()
    );

    let answer = engine
        .send_request(
            "workspace/symbol",
            serde_json::json!({ "query": "indexed" }),
        )
        .await
        .expect("an answer");
    assert_eq!(answer["result"], serde_json::json!([]));
    let busy: prod_code_protocol::readiness::Busy =
        serde_json::from_value(answer[prod_code_protocol::readiness::BUSY_MEMBER].clone())
            .expect("the note of how far it got");
    assert_eq!(
        (
            busy.title.as_str(),
            busy.message.as_deref(),
            busy.percentage
        ),
        ("indexing", Some("1/2"), Some(50))
    );
}

/// Writes the server and a workspace for it to serve.
fn workspace() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = dir.path().join("server.py");
    std::fs::write(&script, SERVER).expect("write the server");
    std::fs::write(dir.path().join("a.txt"), "hello\n").expect("a file to open");
    (dir, script)
}

#[cfg(unix)]
async fn assert_process_exits(pid_file: &Path) {
    let pid = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Ok(pid) = std::fs::read_to_string(pid_file) {
                break pid;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the fake server records its pid");
    tokio::time::timeout(Duration::from_secs(2), async {
        while std::process::Command::new("kill")
            .args(["-0", pid.trim()])
            .status()
            .is_ok_and(|status| status.success())
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the owned fake server exits");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_adapter_initializes_a_server_and_matches_answers_to_requests() {
    let (dir, script) = workspace();
    let engine = GenericLspEngine::spawn(dir.path(), config(&script))
        .await
        .expect("the fake server starts");

    // Both `initialize` and `send_request` hand back the whole JSON-RPC envelope rather than
    // the `result` alone — the gateway forwards it onward as it is. Worth pinning, because a
    // caller that reaches for `contents` directly finds nothing and reports an empty answer.
    let answer = engine.initialize().await.expect("initialize is answered");
    assert_eq!(
        answer.pointer("/result/capabilities/hoverProvider"),
        Some(&serde_json::Value::Bool(true)),
        "the adapter returns what the server said it can do: {answer}"
    );
    assert!(engine.is_alive(), "the child is running");
    assert_eq!(
        *engine.capabilities.read().await,
        Some(serde_json::json!({
            "hoverProvider": true,
            "diagnosticProvider": {"interFileDependencies": false}
        }))
    );

    let uri = format!("file://{}/a.txt", dir.path().display());
    let hover = engine
        .send_request(
            "textDocument/hover",
            serde_json::json!({
                "textDocument": { "uri": uri },
                "position": { "line": 0, "character": 0 }
            }),
        )
        .await
        .expect("the hover is answered");
    assert_eq!(
        hover
            .pointer("/result/contents/value")
            .and_then(|v| v.as_str()),
        Some("the fake server answered"),
        "the answer was matched to the request: {hover}"
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_initialize_during_first_load_retires_its_owned_server() {
    let (dir, script) = workspace();
    let pid_file = dir.path().join("pid");
    let mut settings = config(&script);
    settings.env.insert(
        "FAKE_INITIALIZE_RESPONSE".into(),
        serde_json::json!({"result": {}}).to_string(),
    );
    settings.env.insert(
        "FAKE_PID_FILE".into(),
        pid_file.to_string_lossy().into_owned(),
    );
    let error = match GenericLspEngine::spawn(dir.path(), settings).await {
        Ok(_) => panic!("a malformed first handshake must be refused"),
        Err(error) => error,
    };
    assert!(format!("{error:#}").contains("capabilities"), "{error:#}");
    assert_process_exits(&pid_file).await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn initialize_timeout_retires_the_retained_owned_server() {
    let (dir, script) = workspace();
    let pid_file = dir.path().join("pid");
    let mut settings = config(&script);
    settings.request_timeout = Duration::from_millis(150);
    settings
        .env
        .insert("FAKE_INITIALIZE_AFTER_FIRST".into(), "silence".into());
    settings.env.insert(
        "FAKE_PID_FILE".into(),
        pid_file.to_string_lossy().into_owned(),
    );
    let engine = GenericLspEngine::spawn(dir.path(), settings)
        .await
        .expect("the first handshake succeeds");
    let error = engine
        .initialize()
        .await
        .expect_err("the retained handshake times out");
    assert!(format!("{error:#}").contains("initialize"), "{error:#}");
    assert!(!engine.is_alive(), "the timed out generation is retired");
    assert!(!engine.accepts_documents());
    assert!(engine.capabilities.read().await.is_none());
    assert_process_exits(&pid_file).await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_initialize_retires_the_retained_owned_server() {
    let (dir, script) = workspace();
    let pid_file = dir.path().join("pid");
    let seen_file = dir.path().join("seen");
    let mut settings = config(&script);
    settings.request_timeout = Duration::from_secs(10);
    settings
        .env
        .insert("FAKE_INITIALIZE_AFTER_FIRST".into(), "silence".into());
    settings.env.insert(
        "FAKE_PID_FILE".into(),
        pid_file.to_string_lossy().into_owned(),
    );
    settings.env.insert(
        "FAKE_SEEN_FILE".into(),
        seen_file.to_string_lossy().into_owned(),
    );
    let engine = std::sync::Arc::new(
        GenericLspEngine::spawn(dir.path(), settings)
            .await
            .expect("the first handshake succeeds"),
    );
    let initializing = {
        let engine = std::sync::Arc::clone(&engine);
        tokio::spawn(async move { engine.initialize().await })
    };
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let count = std::fs::read_to_string(&seen_file)
                .unwrap_or_default()
                .lines()
                .filter(|method| *method == "initialize")
                .count();
            if count == 2 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the retained initialize request is owned by the server");
    initializing.abort();
    assert!(initializing.await.expect_err("cancelled").is_cancelled());
    assert!(!engine.is_alive(), "the cancelled generation is retired");
    assert!(!engine.accepts_documents());
    assert!(engine.capabilities.read().await.is_none());
    assert_process_exits(&pid_file).await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn empty_capabilities_are_valid_but_bad_initialize_retires_its_owned_server() {
    let (dir, script) = workspace();
    let mut empty = config(&script);
    empty
        .env
        .insert("FAKE_EMPTY_CAPABILITIES".into(), "1".into());
    let engine = GenericLspEngine::spawn(dir.path(), empty)
        .await
        .expect("an empty capability object is valid");
    engine
        .send_request("textDocument/hover", serde_json::json!({}))
        .await
        .expect("an empty capability object still permits queries");
    drop(engine);

    for response in [
        serde_json::json!({"error":{"code":-32000,"message":"refused"}}),
        serde_json::json!({"result":{"capabilities":{}},"error":{"code":-32000,"message":"also refused"}}),
        serde_json::json!({}),
        serde_json::json!({"result":null}),
        serde_json::json!({"result":[]}),
        serde_json::json!({"result":{}}),
        serde_json::json!({"result":{"capabilities":null}}),
        serde_json::json!({"result":{"capabilities":[]}}),
    ] {
        let (dir, script) = workspace();
        let pid_file = dir.path().join("pid");
        let seen_file = dir.path().join("seen");
        let mut settings = config(&script);
        settings
            .env
            .insert("FAKE_INITIALIZE_AFTER_FIRST".into(), response.to_string());
        settings.env.insert(
            "FAKE_PID_FILE".into(),
            pid_file.to_string_lossy().into_owned(),
        );
        settings.env.insert(
            "FAKE_SEEN_FILE".into(),
            seen_file.to_string_lossy().into_owned(),
        );
        let engine = std::sync::Arc::new(
            GenericLspEngine::spawn(dir.path(), settings)
                .await
                .expect("the initial valid handshake starts the server"),
        );
        let waiting = {
            let engine = std::sync::Arc::clone(&engine);
            tokio::spawn(async move {
                engine
                    .send_request("prodCode/silence", serde_json::json!({}))
                    .await
            })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        let error = engine
            .initialize()
            .await
            .expect_err("an invalid initialize envelope is refused");
        assert!(format!("{error:#}").contains("initializ"), "{error:#}");
        assert!(!engine.is_alive());
        assert!(!engine.accepts_documents());
        let waiting_error = tokio::time::timeout(Duration::from_secs(2), waiting)
            .await
            .expect("the retired generation wakes pending requests")
            .expect("request task")
            .expect_err("a pending request does not survive retirement");
        assert!(format!("{waiting_error:#}").contains("prodCode/silence"));
        let refused = engine
            .send_request("textDocument/hover", serde_json::json!({}))
            .await
            .expect_err("the retained engine refuses later requests");
        assert!(format!("{refused:#}").contains("exited before request"));
        let seen = std::fs::read_to_string(seen_file).expect("request log");
        assert_eq!(
            seen.lines()
                .filter(|method| *method == "initialized")
                .count(),
            1,
            "{seen}"
        );
        assert_process_exits(&pid_file).await;
    }
}

/// A server that checks with fallback settings reports nothing on the probe line until it
/// has the project's build settings; the wait ends at the first error on that line, and gives
/// up at the timeout when none comes (#295).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_wait_for_a_semantic_check_ends_at_the_first_error_on_the_probe_line() {
    let (dir, script) = workspace();
    let path = dir.path().join("Sources/Shop/main.swift");
    let text = "print(1)\nlet __prodCodeProbe: Int = \"\"\n";

    let mut settles = config(&script);
    settles
        .env
        .insert("FAKE_SEMANTIC_AFTER".to_string(), "3".to_string());
    let engine = GenericLspEngine::spawn(dir.path(), settles)
        .await
        .expect("the fake server starts");
    let started = std::time::Instant::now();
    assert_eq!(
        engine
            .wait_for_semantic_check(&path, "swift", text, 1, Duration::from_secs(20))
            .await,
        Ok(true)
    );
    assert!(
        started.elapsed() >= Duration::from_millis(500),
        "two empty answers came first: {:?}",
        started.elapsed()
    );

    let mut never = config(&script);
    never
        .env
        .insert("FAKE_SEMANTIC_AFTER".to_string(), "1000".to_string());
    let engine = GenericLspEngine::spawn(dir.path(), never)
        .await
        .expect("the fake server starts");
    let started = std::time::Instant::now();
    assert_eq!(
        engine
            .wait_for_semantic_check(&path, "swift", text, 1, Duration::from_secs(1))
            .await,
        Ok(false),
        "the server reported on the probe, and found nothing on its line"
    );
    assert!(started.elapsed() < Duration::from_secs(5));
}

/// A server that neither answers a pull nor publishes for the probe never reported on it: the
/// wait says so rather than that the probe found no error (#471).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_semantic_check_the_server_never_reports_on_is_an_error() {
    let (dir, script) = workspace();
    let path = dir.path().join("Sources/Shop/main.swift");
    let mut pushes = config(&script);
    pushes
        .env
        .insert("FAKE_NO_PULL".to_string(), "1".to_string());
    let engine = GenericLspEngine::spawn(dir.path(), pushes)
        .await
        .expect("the fake server starts");
    let err = engine
        .wait_for_semantic_check(&path, "swift", "// silent\n", 1, Duration::from_secs(1))
        .await
        .expect_err("nothing was reported on the probe");
    assert_eq!(
        err.kind,
        Unavailable::NotPublished { sent: Some(1) },
        "{err}"
    );
    assert!(err.uri.ends_with("Sources/Shop/main.swift"), "{err}");

    // A server that exited ends the wait at once.
    let _ = engine
        .send_notification("prodCode/die", serde_json::json!({}))
        .await;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline && engine.is_alive() {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let started = std::time::Instant::now();
    let err = engine
        .wait_for_semantic_check(&path, "swift", "// silent\n", 1, Duration::from_secs(30))
        .await
        .expect_err("an exited server reports nothing");
    assert_eq!(err.kind, Unavailable::ServerExited, "{err}");
    assert!(started.elapsed() < Duration::from_secs(10));
    let err = engine
        .wait_for_semantic_check(
            Path::new("relative.swift"),
            "swift",
            "",
            0,
            Duration::from_secs(1),
        )
        .await
        .expect_err("a path that is no file URI is never opened");
    assert_eq!(err.kind, Unavailable::NeverPublished, "{err}");
}

/// A server that answers a diagnostic pull is asked, whether or not it advertised one
/// (sourcekit-lsp does not, and publishes an empty list ahead of the real one); one that does not
/// know the method gives `None`, and its publications answer instead (#293).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn diagnostics_are_pulled_from_a_server_that_answers_a_pull() {
    let (dir, script) = workspace();
    let uri = format!("file://{}/a.swift", dir.path().display());
    let pulls = GenericLspEngine::spawn(dir.path(), config(&script))
        .await
        .expect("the fake server starts");
    let pulled = pulls
        .pull_diagnostics(&uri)
        .await
        .expect("a pull is answered");
    assert_eq!(pulled[0]["message"].as_str(), Some("pulled"));

    let mut refusing = config(&script);
    refusing
        .env
        .insert("FAKE_NO_PULL".to_string(), "1".to_string());
    let pushes = GenericLspEngine::spawn(dir.path(), refusing)
        .await
        .expect("the fake server starts");
    assert!(pushes.pull_diagnostics(&uri).await.is_none());
    assert!(
        pushes.pull_diagnostics(&uri).await.is_none(),
        "and it is not asked again"
    );
}

/// A publication for the text before the last change can arrive after the change; the answer
/// for the document waits for the one that covers the text last sent (#293).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_diagnostics_answered_are_for_the_text_last_sent() {
    let (dir, script) = workspace();
    let engine = GenericLspEngine::spawn(dir.path(), config(&script))
        .await
        .expect("the fake server starts");
    let uri = format!("file://{}/a.c", dir.path().display());
    engine
        .send_notification(
            "textDocument/didOpen",
            serde_json::json!({ "textDocument": {
                "uri": uri, "languageId": "c", "version": 1, "text": "int a;\n"
            }}),
        )
        .await
        .expect("didOpen is sent");
    let opened = engine
        .current_diagnostics_for(&uri, Duration::from_secs(10))
        .await
        .expect("the publication after the didOpen");
    assert!(
        opened[0]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("disliked"),
        "a publication without a version, after the didOpen, answers for it: {opened:?}"
    );
    engine
        .send_notification(
            "textDocument/didChange",
            serde_json::json!({
                "textDocument": { "uri": uri, "version": 2 },
                "contentChanges": [ { "text": "int b;\n" } ]
            }),
        )
        .await
        .expect("didChange is sent");
    let changed = engine
        .current_diagnostics_for(&uri, Duration::from_secs(10))
        .await
        .expect("the publication for version 2");
    assert_eq!(
        changed[0]["message"].as_str(),
        Some("from the text as changed"),
        "{changed:?}"
    );
    assert_eq!(
        engine.diagnostics_for(&uri).await.expect("published")[0]["message"].as_str(),
        Some("from the text as changed")
    );

    // A document nothing was sent for waits a short while for a first publication, and then
    // has no report, which is not a clean one.
    let started = std::time::Instant::now();
    let none = engine
        .current_diagnostics_for("file:///never/opened.c", Duration::from_secs(60))
        .await
        .expect_err("nothing was published for it");
    assert_eq!(none.kind, Unavailable::NeverPublished, "{none}");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "waited {:?}",
        started.elapsed()
    );

    // What was published for a document is dropped when it is closed: the next session to
    // open it numbers its versions from 1 again, and a publication for this one's version 2
    // would pass for its own. What comes after the close is the server's clearing, empty.
    engine
        .send_notification(
            "textDocument/didClose",
            serde_json::json!({ "textDocument": { "uri": uri } }),
        )
        .await
        .expect("didClose is sent");
    assert!(
        engine
            .diagnostics_for(&uri)
            .await
            .unwrap_or_default()
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_late_older_publication_does_not_replace_a_newer_one() {
    let (dir, script) = workspace();
    let engine = GenericLspEngine::spawn(dir.path(), config(&script))
        .await
        .expect("the fake server starts");
    let uri = format!("file://{}/ordered.c", dir.path().display());
    open(&engine, &uri, "// silent\n").await;
    engine
        .send_notification(
            "textDocument/didChange",
            serde_json::json!({
                "textDocument": { "uri": uri, "version": 2 },
                "contentChanges": [ { "text": "// stale-only\n" } ]
            }),
        )
        .await
        .expect("didChange is sent");

    engine
        .send_request(
            "prodCode/publishTwoVersions",
            serde_json::json!({
                "uri": uri,
                "first": 2,
                "firstMessage": "newer publication",
                "second": 1,
                "secondMessage": "late older publication"
            }),
        )
        .await
        .expect("both publications were sent before this answer");

    let current = engine
        .current_diagnostics_for(&uri, Duration::from_millis(20))
        .await
        .expect("the newer publication remains current");
    assert_eq!(current[0]["message"], "newer publication");
    assert_eq!(
        engine.diagnostics_for(&uri).await.expect("published")[0]["message"],
        "newer publication"
    );

    engine
        .send_notification(
            "textDocument/didClose",
            serde_json::json!({ "textDocument": { "uri": uri } }),
        )
        .await
        .expect("didClose is sent");
    open(&engine, &uri, "// silent reopened\n").await;
    engine
        .send_request(
            "prodCode/publishTwoVersions",
            serde_json::json!({
                "uri": uri,
                "first": 2,
                "firstMessage": "stale closed generation",
                "second": 1,
                "secondMessage": "valid reset publication"
            }),
        )
        .await
        .expect("both publications were sent before this answer");
    let reopened = engine
        .current_diagnostics_for(&uri, Duration::from_millis(20))
        .await
        .expect("the reopened document accepts its lower reset version");
    assert_eq!(reopened[0]["message"], "valid reset publication");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_published_diagnostic_is_kept_for_the_file_it_belongs_to() {
    let (dir, script) = workspace();
    let engine = GenericLspEngine::spawn(dir.path(), config(&script))
        .await
        .expect("the fake server starts");
    engine.initialize().await.expect("initialize");

    let uri = format!("file://{}/a.txt", dir.path().display());
    engine
        .send_notification(
            "textDocument/didOpen",
            serde_json::json!({ "textDocument": {
                "uri": uri, "languageId": "plaintext", "version": 1, "text": "hello\n"
            }}),
        )
        .await
        .expect("didOpen is sent");

    // The server publishes on open; give the reader a moment to take it off the pipe.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline && !engine.diagnostics_published(&uri).await {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        engine.diagnostics_published(&uri).await,
        "the publication was noticed"
    );
    let items = engine.diagnostics_for(&uri).await.expect("published");
    assert_eq!(items.len(), 1, "one diagnostic, for this file: {items:?}");
    assert!(
        items[0]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("disliked"),
        "and it is the one the server sent: {items:?}"
    );
    assert!(
        engine
            .diagnostics_for("file:///somewhere/else.txt")
            .await
            .is_none(),
        "another file has none"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_the_server_never_answers_comes_back_as_a_timeout() {
    let (dir, script) = workspace();
    let mut settings = config(&script);
    settings.request_timeout = Duration::from_millis(400);
    let engine = GenericLspEngine::spawn(dir.path(), settings)
        .await
        .expect("the fake server starts");
    engine.initialize().await.expect("initialize");

    let err = engine
        .send_request("prodCode/silence", serde_json::json!({}))
        .await
        .expect_err("silence is not an answer");
    let text = format!("{err:#}").to_lowercase();
    assert!(
        text.contains("timeout") || text.contains("timed out"),
        "the failure says what happened: {text}"
    );
    assert!(
        engine.is_alive(),
        "and the server is still there for the next request"
    );
}

/// A request deadline covers pipe backpressure as well as the response wait (#533).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_whose_full_frame_cannot_be_written_obeys_its_deadline() {
    let (dir, script) = workspace();
    let mut settings = config(&script);
    settings
        .env
        .insert("FAKE_STOP_READING".to_string(), "1".to_string());
    settings.request_timeout = Duration::from_millis(200);
    let engine = GenericLspEngine::spawn(dir.path(), settings)
        .await
        .expect("the fake server starts");

    let observed = tokio::time::timeout(
        Duration::from_secs(2),
        engine.send_request(
            "prodCode/large",
            serde_json::json!({ "payload": "x".repeat(8 * 1024 * 1024) }),
        ),
    )
    .await;
    let result = observed.expect("the configured request deadline includes a blocked write");
    let error = result.expect_err("the server stopped reading before the frame fit");
    let text = format!("{error:#}");
    assert!(text.contains("prodCode/large"), "{text}");
    assert!(text.to_lowercase().contains("timeout"), "{text}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notification_contention_retires_the_partial_stream_without_a_delayed_request() {
    let (dir, script) = workspace();
    let mut settings = config(&script);
    settings
        .env
        .insert("FAKE_SLOW_READ".to_string(), "0.01".to_string());
    settings.request_timeout = Duration::from_secs(2);
    let seen_file = dir.path().join("seen");
    let reading_file = dir.path().join("reading");
    settings.env.insert(
        "FAKE_SEEN_FILE".to_string(),
        seen_file.to_string_lossy().into_owned(),
    );
    settings.env.insert(
        "FAKE_READING_FILE".to_string(),
        reading_file.to_string_lossy().into_owned(),
    );
    let engine = std::sync::Arc::new(
        GenericLspEngine::spawn(dir.path(), settings)
            .await
            .expect("the fake server starts"),
    );
    let _ = std::fs::remove_file(&reading_file);

    let writing = {
        let engine = std::sync::Arc::clone(&engine);
        tokio::spawn(async move {
            engine
                .send_notification(
                    "prodCode/largeNotification",
                    serde_json::json!({ "payload": "x".repeat(32 * 1024 * 1024) }),
                )
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        while !reading_file.exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the large notification owns the writer");
    let request_error = engine
        .send_request("prodCode/queued", serde_json::json!({}))
        .await
        .expect_err("the partial notification retires the server");
    let notification_error = writing
        .await
        .expect("the notification task")
        .expect_err("the notification reaches its bounded deadline");
    let text = format!("{notification_error:#}");
    assert!(text.contains("prodCode/largeNotification"), "{text}");
    assert!(text.to_lowercase().contains("timeout"), "{text}");
    assert!(
        format!("{request_error:#}").contains("prodCode/queued"),
        "{request_error:#}"
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    let seen = std::fs::read_to_string(seen_file).expect("request log");
    assert!(
        !seen.lines().any(|method| method == "prodCode/queued"),
        "the expired request was written after its caller returned: {seen}"
    );
    assert!(!engine.is_alive());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_write_error_names_the_request_and_retires_the_child() {
    let (dir, script) = workspace();
    let mut settings = config(&script);
    settings
        .env
        .insert("FAKE_CLOSE_STDIN".to_string(), "1".to_string());
    settings.request_timeout = Duration::from_secs(2);
    let engine = GenericLspEngine::spawn(dir.path(), settings)
        .await
        .expect("the fake server starts");
    tokio::time::sleep(Duration::from_millis(100)).await;

    let error = engine
        .send_request(
            "prodCode/brokenWrite",
            serde_json::json!({ "payload": "x".repeat(1024 * 1024) }),
        )
        .await
        .expect_err("the server closed its read end");
    let text = format!("{error:#}");
    assert!(text.contains("prodCode/brokenWrite"), "{text}");
    assert!(text.to_lowercase().contains("write"), "{text}");
    assert!(!engine.is_alive(), "a possibly partial stream is retired");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_a_partial_frame_retires_only_that_child_and_wakes_waiters() {
    let (dir, script) = workspace();
    let mut settings = config(&script);
    settings
        .env
        .insert("FAKE_STOP_READING".to_string(), "1".to_string());
    settings.request_timeout = Duration::from_secs(10);
    let engine = std::sync::Arc::new(
        GenericLspEngine::spawn(dir.path(), settings)
            .await
            .expect("the fake server starts"),
    );

    let writing = {
        let engine = std::sync::Arc::clone(&engine);
        tokio::spawn(async move {
            engine
                .send_request(
                    "prodCode/cancelled",
                    serde_json::json!({ "payload": "x".repeat(8 * 1024 * 1024) }),
                )
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    let waiting = {
        let engine = std::sync::Arc::clone(&engine);
        tokio::spawn(async move {
            engine
                .send_request("prodCode/waiting", serde_json::json!({}))
                .await
        })
    };
    writing.abort();
    assert!(
        writing
            .await
            .expect_err("the write was cancelled")
            .is_cancelled()
    );
    let error = tokio::time::timeout(Duration::from_secs(2), waiting)
        .await
        .expect("retirement wakes a request waiting behind the writer")
        .expect("the waiting task")
        .expect_err("the retired child cannot answer");
    assert!(
        format!("{error:#}").contains("prodCode/waiting"),
        "{error:#}"
    );
    assert!(!engine.is_alive(), "the desynchronized child was retired");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn completed_frames_support_cancellation_and_concurrent_healthy_responses() {
    let (dir, script) = workspace();
    let engine = std::sync::Arc::new(
        GenericLspEngine::spawn(dir.path(), config(&script))
            .await
            .expect("the fake server starts"),
    );

    let cancelled = {
        let engine = std::sync::Arc::clone(&engine);
        tokio::spawn(async move {
            engine
                .send_request("prodCode/delay", serde_json::json!({}))
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    cancelled.abort();
    assert!(cancelled.await.expect_err("cancelled").is_cancelled());

    let first = {
        let engine = std::sync::Arc::clone(&engine);
        tokio::spawn(async move {
            engine
                .send_request("textDocument/hover", serde_json::json!({}))
                .await
        })
    };
    let second = {
        let engine = std::sync::Arc::clone(&engine);
        tokio::spawn(async move {
            engine
                .send_request("textDocument/hover", serde_json::json!({}))
                .await
        })
    };
    for response in [first, second] {
        assert_eq!(
            response.await.expect("task").expect("healthy response")["result"]["contents"]["value"],
            "the fake server answered"
        );
    }
    assert!(
        engine.is_alive(),
        "complete-frame cancellation keeps the child"
    );
}

/// A request in flight when the server exits fails at once, saying so, instead of at the
/// request timeout (#355): a TypeScript server that crashed on a file kept its caller 30 s.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_in_flight_fails_as_soon_as_the_server_exits() {
    let (dir, script) = workspace();
    let engine = std::sync::Arc::new(
        GenericLspEngine::spawn(dir.path(), config(&script))
            .await
            .expect("the fake server starts"),
    );
    engine.initialize().await.expect("initialize");

    let waiting = {
        let engine = std::sync::Arc::clone(&engine);
        tokio::spawn(async move {
            engine
                .send_request("prodCode/silence", serde_json::json!({}))
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(200)).await;
    let _ = engine
        .send_notification("prodCode/die", serde_json::json!({}))
        .await;

    let answer = tokio::time::timeout(Duration::from_secs(10), waiting)
        .await
        .expect("the request ends well before its 30 s timeout")
        .expect("the request's task");
    let err = answer.expect_err("a server that exited answers nothing");
    assert!(format!("{err:#}").contains("has exited"), "{err:#}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_server_that_exits_is_noticed() {
    let (dir, script) = workspace();
    let engine = GenericLspEngine::spawn(dir.path(), config(&script))
        .await
        .expect("the fake server starts");
    engine.initialize().await.expect("initialize");
    assert!(engine.is_alive());

    let _ = engine
        .send_notification("prodCode/die", serde_json::json!({}))
        .await;

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline && engine.is_alive() {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        !engine.is_alive(),
        "a server that exited is not reported as running"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn idleness_is_measured_from_the_last_request() {
    let (dir, script) = workspace();
    let engine = GenericLspEngine::spawn(dir.path(), config(&script))
        .await
        .expect("the fake server starts");
    engine.initialize().await.expect("initialize");

    tokio::time::sleep(Duration::from_millis(300)).await;
    let idle_before = engine.idle_duration().await;
    assert!(
        idle_before >= Duration::from_millis(250),
        "it has been idle for a while: {idle_before:?}"
    );

    let uri = format!("file://{}/a.txt", dir.path().display());
    engine
        .send_request(
            "textDocument/hover",
            serde_json::json!({
                "textDocument": { "uri": uri },
                "position": { "line": 0, "character": 0 }
            }),
        )
        .await
        .expect("hover");
    let idle_after = engine.idle_duration().await;
    assert!(
        idle_after < idle_before,
        "a request resets the clock: {idle_after:?} then {idle_before:?}"
    );
}

#[tokio::test]
async fn a_server_that_cannot_be_started_is_refused_by_name() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut settings = GenericLspConfig {
        command: "prod-code-no-such-language-server".to_string(),
        ..Default::default()
    };
    settings.args.clear();
    match GenericLspEngine::spawn(dir.path(), settings).await {
        Ok(_) => panic!("there is no such binary"),
        Err(err) => {
            let text = format!("{err:#}");
            assert!(
                text.contains("prod-code-no-such-language-server"),
                "the failure names what it tried to start: {text}"
            );
        }
    }
}

/// Who decides that a server has been idle long enough is the caller, not the adapter: the
/// gateway evicts a whole workspace, engine included. The adapter's part is to say how long it
/// has been, which is what this holds.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_adapter_reports_idleness_and_leaves_the_decision_to_its_caller() {
    let (dir, script) = workspace();
    let engine = GenericLspEngine::spawn(dir.path(), config(&script))
        .await
        .expect("the fake server starts");
    engine.initialize().await.expect("initialize");

    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(
        engine.idle_duration().await >= Duration::from_millis(350),
        "the idle clock runs"
    );
    assert!(
        engine.is_alive(),
        "and nothing in the adapter stops the server on its own"
    );
}

/// The configurations the gateway builds per language, and the lookups they rest on.
#[test]
fn each_language_gets_a_configuration_that_names_its_server() {
    let python = GenericLspConfig::for_python();
    assert!(
        python.command.contains("pyright") || python.command.contains("python"),
        "Python's server: {}",
        python.command
    );
    let cpp = GenericLspConfig::for_cpp();
    assert!(
        cpp.command.contains("clangd"),
        "C++'s server: {}",
        cpp.command
    );
    assert!(
        cpp.args.iter().any(|a| a == "--use-dirty-headers"),
        "clangd must parse an open header's proposed text, not the file on disk (#292): {:?}",
        cpp.args
    );
    let validation = GenericLspConfig::for_cpp_validation();
    assert_eq!(validation.command, cpp.command);
    assert!(
        validation
            .args
            .iter()
            .any(|a| a == "--background-index=false")
            && !validation.args.iter().any(|a| a == "--background-index")
            && validation.args.iter().any(|a| a == "--use-dirty-headers"),
        "the validation server indexes nothing in the background: {:?}",
        validation.args
    );
    let swift = GenericLspConfig::for_swift();
    assert!(
        swift.command.contains("sourcekit"),
        "Swift's server: {}",
        swift.command
    );
    let typescript = GenericLspConfig::for_typescript();
    assert!(
        !typescript.command.is_empty(),
        "TypeScript's server is named"
    );

    // Every one of them waits the default for an answer unless told otherwise.
    for config in [&python, &cpp, &swift, &typescript] {
        assert_eq!(
            config.request_timeout,
            prod_code_engine_generic::DEFAULT_REQUEST_TIMEOUT,
            "{} keeps the default budget",
            config.command
        );
    }
}

#[test]
fn a_binary_that_is_not_installed_is_reported_by_name() {
    let err = prod_code_engine_generic::which_bin("prod-code-no-such-binary-anywhere")
        .expect_err("it is not installed");
    assert!(
        format!("{err:#}").contains("prod-code-no-such-binary-anywhere"),
        "the failure names what was looked for: {err:#}"
    );
    assert!(
        prod_code_engine_generic::which_bin("sh").is_ok(),
        "and a binary that is installed is found"
    );
}

#[test]
fn a_virtual_environment_is_used_when_the_project_has_one() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert!(
        prod_code_engine_generic::venv_python(dir.path()).is_none(),
        "a project without one has none"
    );
    let bin = dir.path().join(".venv").join("bin");
    std::fs::create_dir_all(&bin).expect("venv dir");
    std::fs::write(bin.join("python"), "#!/bin/sh\nexit 0\n").expect("python");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(bin.join("python"), std::fs::Permissions::from_mode(0o755))
            .expect("chmod");
    }
    let found = prod_code_engine_generic::venv_python(dir.path()).expect("the venv is found");
    assert!(
        found.contains(".venv"),
        "and it is the project's own: {found}"
    );
}

#[test]
fn settings_are_read_from_the_project_for_the_section_that_asks() {
    let dir = tempfile::tempdir().expect("tempdir");
    let empty = prod_code_engine_generic::settings_for_section(dir.path(), "python");
    assert!(
        empty.is_object() || empty.is_null(),
        "a project with no settings gives something harmless: {empty}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_command_that_asks_for_an_edit_hands_the_edit_back() {
    let (dir, script) = workspace();
    let engine = GenericLspEngine::spawn(dir.path(), config(&script))
        .await
        .expect("the fake server starts");
    engine.initialize().await.expect("initialize");

    // A server runs a command and, while it runs, asks the client to apply an edit. The
    // adapter's job is to catch that request and return the edit rather than let it vanish.
    let edit = engine
        .execute_command_capturing_edit(serde_json::json!({
            "command": "prodCode/edit",
            "arguments": []
        }))
        .await
        .expect("the command runs")
        .expect("an edit was captured");
    assert!(
        edit.to_string().contains("edited"),
        "the edit the server asked for: {edit}"
    );

    // A command that answers with an error is an error here, not an empty edit.
    let err = engine
        .execute_command_capturing_edit(serde_json::json!({
            "command": "prodCode/refuse",
            "arguments": []
        }))
        .await
        .expect_err("the server refused");
    assert!(
        format!("{err:#}").contains("refused"),
        "the server's own message comes through: {err:#}"
    );

    // And one that neither errors nor asks for anything comes back as no edit.
    let nothing = engine
        .execute_command_capturing_edit(serde_json::json!({
            "command": "prodCode/quiet",
            "arguments": []
        }))
        .await
        .expect("the command runs");
    assert!(nothing.is_none(), "no edit was asked for: {nothing:?}");
}

#[test]
fn an_engine_reports_its_server_only_when_one_is_installed() {
    // A language nobody has a server for is never reported as ready.
    assert_eq!(GenericLspConfig::installed_server("cobol"), None);

    // For the ones this machine has, the label names the binary that would run.
    for engine in ["cpp", "python", "typescript", "swift"] {
        if let Some(label) = GenericLspConfig::installed_server(engine) {
            assert!(
                !label.trim().is_empty(),
                "{engine} reports a label when it is installed"
            );
        }
    }
}

#[test]
fn python_settings_follow_the_project_into_its_virtual_environment() {
    let dir = tempfile::tempdir().expect("tempdir");

    // The analysis section is the same wherever it is asked for.
    let analysis = prod_code_engine_generic::settings_for_section(dir.path(), "python.analysis");
    assert_eq!(
        analysis.get("diagnosticMode").and_then(|m| m.as_str()),
        Some("workspace"),
        "the analysis settings are handed over whole: {analysis}"
    );

    // Without a virtual environment, the interpreter is left to the server to find.
    let plain = prod_code_engine_generic::settings_for_section(dir.path(), "python");
    assert!(
        plain.get("pythonPath").is_none(),
        "nothing to point at yet: {plain}"
    );

    let bin = dir.path().join(".venv").join("bin");
    std::fs::create_dir_all(&bin).expect("venv dir");
    std::fs::write(bin.join("python"), "#!/bin/sh\nexit 0\n").expect("python");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(bin.join("python"), std::fs::Permissions::from_mode(0o755))
            .expect("chmod");
    }
    let with_venv = prod_code_engine_generic::settings_for_section(dir.path(), "basedpyright");
    assert!(
        with_venv
            .get("pythonPath")
            .and_then(|p| p.as_str())
            .is_some_and(|p| p.contains(".venv")),
        "the project's own interpreter is pointed at: {with_venv}"
    );
    assert_eq!(
        with_venv.get("venv").and_then(|v| v.as_str()),
        Some(".venv")
    );

    // A section nothing knows about gets an empty object rather than a guess.
    let unknown = prod_code_engine_generic::settings_for_section(dir.path(), "ruby");
    assert_eq!(unknown, serde_json::json!({}));
}

/// Opens `text` as version 1 of the document at `uri`.
async fn open(engine: &GenericLspEngine, uri: &str, text: &str) {
    engine
        .send_notification(
            "textDocument/didOpen",
            serde_json::json!({ "textDocument": {
                "uri": uri, "languageId": "c", "version": 1, "text": text
            }}),
        )
        .await
        .expect("didOpen is sent");
}

/// A server that published nothing for the text opened has no report on it, and says so by
/// document and version instead of answering with an empty list (#471); one that exited says
/// that at once rather than at the end of the wait.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_document_the_server_published_nothing_for_has_no_report() {
    let (dir, script) = workspace();
    let engine = GenericLspEngine::spawn(dir.path(), config(&script))
        .await
        .expect("the fake server starts");
    let uri = format!("file://{}/silent.c", dir.path().display());
    open(&engine, &uri, "// silent\n").await;
    let err = engine
        .current_diagnostics_for(&uri, Duration::from_millis(20))
        .await
        .expect_err("nothing was published for version 1");
    assert_eq!(
        err.kind,
        Unavailable::NotPublished { sent: Some(1) },
        "{err}"
    );
    assert_eq!(err.uri, uri);
    assert!(
        err.to_string().contains("(version 1)") && err.waited >= Duration::from_millis(20),
        "{err}"
    );

    let _ = engine
        .send_notification("prodCode/die", serde_json::json!({}))
        .await;
    let started = std::time::Instant::now();
    let err = engine
        .current_diagnostics_for(&uri, Duration::from_secs(60))
        .await
        .expect_err("an exited server publishes nothing");
    assert_eq!(err.kind, Unavailable::ServerExited, "{err}");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
}

/// The build of version 1, published after version 2 was sent and never followed by one for
/// version 2, is not version 2's report (#471).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_publication_for_an_older_version_is_not_the_report_of_the_newer_one() {
    let (dir, script) = workspace();
    let engine = GenericLspEngine::spawn(dir.path(), config(&script))
        .await
        .expect("the fake server starts");
    let uri = format!("file://{}/a.c", dir.path().display());
    open(&engine, &uri, "int a;\n").await;
    engine
        .current_diagnostics_for(&uri, Duration::from_secs(10))
        .await
        .expect("the publication after the didOpen");
    engine
        .send_notification(
            "textDocument/didChange",
            serde_json::json!({
                "textDocument": { "uri": uri, "version": 2 },
                "contentChanges": [ { "text": "int b; // stale-only\n" } ]
            }),
        )
        .await
        .expect("didChange is sent");
    let err = engine
        .current_diagnostics_for(&uri, Duration::from_millis(600))
        .await
        .expect_err("only version 1 was published");
    assert_eq!(
        err.kind,
        Unavailable::OlderVersion {
            published: 1,
            sent: 2
        },
        "{err}"
    );
    assert_eq!(
        engine.diagnostics_for(&uri).await.expect("published")[0]["message"],
        "from the text before the change",
        "what was published is still there, for what it is"
    );
}

/// Before the server has published for the text opened there is no report; once it has, its
/// publication answers, and an empty one is a clean report.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_late_publication_answers_and_an_empty_one_is_a_clean_report() {
    let (dir, script) = workspace();
    let engine = GenericLspEngine::spawn(dir.path(), config(&script))
        .await
        .expect("the fake server starts");
    let uri = format!("file://{}/clean.c", dir.path().display());
    let started = std::time::Instant::now();
    open(&engine, &uri, "// late-clean\n").await;
    let early = engine
        .current_diagnostics_for(&uri, Duration::from_millis(20))
        .await
        .expect_err("the server has not published yet");
    assert_eq!(
        early.kind,
        Unavailable::NotPublished { sent: Some(1) },
        "{early}"
    );
    let late = engine
        .current_diagnostics_for(&uri, Duration::from_secs(10))
        .await
        .expect("the publication for version 1");
    assert!(late.is_empty(), "{late:?}");
    assert!(
        started.elapsed() >= Duration::from_millis(300),
        "it was the late publication that answered: {:?}",
        started.elapsed()
    );
}

/// Closing a document drops what was published for it; clangd's clearing after the close,
/// without a version, is no report, and neither is the silence after a reopen, while the
/// reopened text's own publication is.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_closed_or_reopened_document_is_not_answered_from_before() {
    let (dir, script) = workspace();
    let engine = GenericLspEngine::spawn(dir.path(), config(&script))
        .await
        .expect("the fake server starts");
    let uri = format!("file://{}/a.c", dir.path().display());
    open(&engine, &uri, "int a;\n").await;
    engine
        .send_notification(
            "textDocument/didChange",
            serde_json::json!({
                "textDocument": { "uri": uri, "version": 2 },
                "contentChanges": [ { "text": "int b;\n" } ]
            }),
        )
        .await
        .expect("didChange is sent");
    let changed = engine
        .current_diagnostics_for(&uri, Duration::from_secs(10))
        .await
        .expect("the publication for version 2");
    assert_eq!(changed[0]["message"], "from the text as changed");

    engine
        .send_notification(
            "textDocument/didClose",
            serde_json::json!({ "textDocument": { "uri": uri } }),
        )
        .await
        .expect("didClose is sent");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline && !engine.diagnostics_published(&uri).await {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(engine.diagnostics_for(&uri).await, Some(Vec::new()));
    let closed = engine
        .current_diagnostics_for(&uri, Duration::from_millis(20))
        .await
        .expect_err("a closed document's clearing is no report");
    assert_eq!(
        closed.kind,
        Unavailable::Unversioned { sent: None },
        "{closed}"
    );

    open(&engine, &uri, "// silent\n").await;
    let reopened = engine
        .current_diagnostics_for(&uri, Duration::from_millis(20))
        .await
        .expect_err("nothing was published for the reopened text");
    assert_eq!(
        reopened.kind,
        Unavailable::NotPublished { sent: Some(1) },
        "version 2's errors do not answer for the new version 1: {reopened}"
    );

    engine
        .send_notification(
            "textDocument/didClose",
            serde_json::json!({ "textDocument": { "uri": uri } }),
        )
        .await
        .expect("didClose is sent");
    // The server numbers its publications by now, so the reopened text's own is numbered too;
    // the close's clearing may still arrive after the reopen, and is passed over.
    open(&engine, &uri, "// late-clean\n").await;
    let fresh = engine
        .current_diagnostics_for(&uri, Duration::from_secs(10))
        .await
        .expect("the reopened text's publication");
    assert!(fresh.is_empty(), "{fresh:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_late_version_from_a_closed_session_does_not_check_reopened_text() {
    let (dir, script) = workspace();
    let engine = GenericLspEngine::spawn(dir.path(), config(&script))
        .await
        .unwrap();
    let uri = "file:///wherever/reopened.c";
    open(&engine, uri, "int old;\n").await;
    engine
        .current_diagnostics_for(uri, Duration::from_secs(5))
        .await
        .unwrap();
    engine
        .send_notification(
            "textDocument/didChange",
            serde_json::json!({
                "textDocument": {"uri": uri, "version": 2},
                "contentChanges": [{"text": "int older;\n"}]
            }),
        )
        .await
        .unwrap();
    // The fake server publishes version 2 after 300 ms, after we close and reopen at 1.
    engine
        .send_notification(
            "textDocument/didClose",
            serde_json::json!({
                "textDocument": {"uri": uri}
            }),
        )
        .await
        .unwrap();
    open(&engine, uri, "// silent new text\n").await;
    let report = engine
        .current_diagnostics_for(uri, Duration::from_millis(650))
        .await;
    assert!(
        report.is_err(),
        "old session version 2 answered for reopened version 1: {report:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn incomplete_pull_reports_are_not_complete_evidence() {
    let cases = [
        serde_json::json!({"result": {"kind": "unchanged", "resultId": "never-cached", "items": []}}),
        serde_json::json!({"result": {"kind": "unknown", "items": []}}),
        serde_json::json!({"result": {"kind": "", "items": []}}),
        serde_json::json!({"result": {"items": []}}),
        serde_json::json!({"result": {"kind": null, "items": []}}),
        serde_json::json!({"result": {"kind": "full"}}),
        serde_json::json!({"result": {"kind": "full", "items": null}}),
        serde_json::json!({"result": {"kind": "full", "items": {}}}),
        serde_json::json!({"error": {"code": -32001, "message": "unavailable"},
            "result": {"kind": "full", "items": []}}),
        serde_json::json!({"error": null, "result": {"kind": "full", "items": []}}),
    ];
    for response in cases {
        let (dir, script) = workspace();
        let mut settings = config(&script);
        settings
            .env
            .insert("FAKE_RESPONSE".into(), response.to_string());
        let engine = GenericLspEngine::spawn(dir.path(), settings).await.unwrap();
        let report = engine.pull_diagnostics("file:///wherever/a.txt").await;
        assert!(
            report.is_none(),
            "{response} became a full report: {report:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_pull_reports_keep_empty_and_error_items() {
    let diagnostic = serde_json::json!({"range": {"start": {"line": 0, "character": 0},
        "end": {"line": 0, "character": 1}}, "severity": 1, "message": "type mismatch"});
    for items in [vec![], vec![diagnostic]] {
        let (dir, script) = workspace();
        let mut settings = config(&script);
        settings.env.insert(
            "FAKE_RESPONSE".into(),
            serde_json::json!({"result": {"kind": "full", "items": items}}).to_string(),
        );
        let engine = GenericLspEngine::spawn(dir.path(), settings).await.unwrap();
        let report = engine.pull_diagnostics("file:///wherever/a.txt").await;
        assert_eq!(report, Some(items));
    }
}

#[tokio::test]
#[ignore = "requires an installed basedpyright language server"]
async fn direct_python_builtin_identity_probe() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let file = root.join("generator.py");
    let source = r####"#!/usr/bin/env python3
"""Regenerate the replication figures in docs/img from the numbers in docs/replication.md.
Dependency-free (hand-written SVG) so the figures are reproducible anywhere."""
import os, textwrap

OUT = os.path.join(os.path.dirname(__file__), "img")
FONT = "font-family='JetBrains Mono, SFMono-Regular, Menlo, monospace'"
INK, MUTED, GRID, PANEL = "#1f2328", "#6a737d", "#d0d7de", "#f6f8fa"
RING, RING_FILL = "#c0392b", "#fdecea"
NET, NET_FILL = "#1d4ed8", "#e8efff"
OK, OK_FILL = "#2e7d32", "#e8f5e9"
OTHER = "#8fa3b8"

def esc(t):
    return str(t).replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")

def para(x, y, text, width=95, color=None, size=12, lh=17):
    col = color or MUTED
    return "".join(f"<text x='{x}' y='{y + i * lh}' fill='{col}' font-size='{size}'>{esc(line)}</text>"
                   for i, line in enumerate(textwrap.wrap(text, width)))

def head(W, H, title, sub=None):
    s = [f"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 {W} {H}' width='{W}' height='{H}' {FONT} font-size='12'>",
         "<defs>"
         f"<marker id='a' markerWidth='8' markerHeight='8' refX='7' refY='4' orient='auto'><path d='M0,0 L8,4 L0,8 z' fill='{INK}'/></marker>"
         f"<marker id='n' markerWidth='8' markerHeight='8' refX='7' refY='4' orient='auto'><path d='M0,0 L8,4 L0,8 z' fill='{NET}'/></marker>"
         f"<marker id='r' markerWidth='8' markerHeight='8' refX='7' refY='4' orient='auto'><path d='M0,0 L8,4 L0,8 z' fill='{RING}'/></marker>"
         "</defs>",
         f"<rect width='{W}' height='{H}' fill='white'/>",
         f"<text x='16' y='24' font-size='15' font-weight='bold' fill='{INK}'>{esc(title)}</text>"]
    if sub:
        s.append(f"<text x='16' y='42' fill='{MUTED}'>{esc(sub)}</text>")
    return s

def box(x, y, w, h, text, sub=None, fill="white", stroke=INK, bold=True, size=12):
    t = f"<rect x='{x}' y='{y}' width='{w}' height='{h}' rx='6' fill='{fill}' stroke='{stroke}' stroke-width='1.5'/>"
    t += f"<text x='{x + w / 2}' y='{y + h / 2 + (-2 if sub else 5)}' text-anchor='middle' fill='{INK}' font-size='{size}' font-weight='{'bold' if bold else 'normal'}'>{esc(text)}</text>"
    if sub:
        t += f"<text x='{x + w / 2}' y='{y + h / 2 + 14}' text-anchor='middle' fill='{MUTED}' font-size='11'>{esc(sub)}</text>"
    return t

def ring(x, y, w, h, text="ring", sub="/dev/shm"):
    return box(x, y, w, h, text, sub, fill=RING_FILL, stroke=RING)

def panel(x, y, w, h, title, fill=PANEL):
    return (f"<rect x='{x}' y='{y}' width='{w}' height='{h}' rx='10' fill='{fill}' stroke='{GRID}'/>"
            f"<text x='{x + 12}' y='{y + 20}' fill='{INK}' font-weight='bold' font-size='13'>{esc(title)}</text>")

def arrow(x1, y1, x2, y2, label=None, color=INK, marker="a", dash=None, above=True, size=11):
    d = f" stroke-dasharray='{dash}'" if dash else ""
    t = f"<line x1='{x1}' y1='{y1}' x2='{x2}' y2='{y2}' stroke='{color}' stroke-width='1.5' marker-end='url(#{marker})'{d}/>"
    if label:
        ly = min(y1, y2) - 7 if above else max(y1, y2) + 15
        t += f"<text x='{(x1 + x2) / 2}' y='{ly}' text-anchor='middle' fill='{color}' font-size='{size}'>{esc(label)}</text>"
    return t

def write(name, s):
    s.append("</svg>")
    open(os.path.join(OUT, name), "w").write("\n".join(s))

# 1. The pipeline of one record: source ring to mirror ring, with measured latencies ----------
W, H = 960, 340
s = head(W, H, "One record's path: source ring to mirror ring, same sequence number everywhere",
         "push on the source host → read on the mirror host, measured, 64-byte records")
s.append(panel(16, 56, 330, 190, "source host"))
s.append(box(30, 96, 86, 44, "producer", "push()"))
s.append(arrow(116, 118, 142, 118))
s.append(ring(142, 90, 96, 56, "ring", "seq 1, 2, 3 …"))
s.append(arrow(238, 104, 262, 104))
s.append(box(262, 86, 74, 36, "serve", "raw reader", size=11))
s.append(arrow(238, 132, 262, 160))
s.append(box(262, 150, 74, 36, "readers", "0.1 µs", size=11))
s.append(f"<text x='181' y='222' text-anchor='middle' fill='{MUTED}' font-size='11'>fixed slots, or descriptors + arena</text>")
# network band
s.append(f"<rect x='356' y='66' width='250' height='170' rx='10' fill='{NET_FILL}' stroke='{NET}' stroke-dasharray='4 3'/>")
s.append(f"<text x='481' y='86' text-anchor='middle' fill='{NET}' font-weight='bold'>network</text>")
s.append(arrow(336, 118, 616, 118, "DATA: raw slot bytes + seq", NET, "n"))
s.append(f"<text x='481' y='146' text-anchor='middle' fill='{NET}' font-size='11'>UDP multicast · UDP unicast · TCP</text>")
s.append(arrow(616, 186, 336, 186, "NAK / GAP over TCP", NET, "n", dash="4 3", above=False))
s.append(f"<text x='481' y='214' text-anchor='middle' fill='{MUTED}' font-size='11'>the source ring is</text>")
s.append(f"<text x='481' y='228' text-anchor='middle' fill='{MUTED}' font-size='11'>the retransmission buffer</text>")
s.append(panel(616, 56, 328, 190, "mirror host"))
s.append(box(630, 100, 84, 36, "mirror", "one writer", size=11))
s.append(arrow(714, 118, 736, 118))
s.append(ring(736, 90, 96, 56, "ring", "same seq"))
s.append(arrow(832, 118, 852, 118))
s.append(box(852, 96, 80, 44, "readers", "as local", size=11))
s.append(f"<text x='780' y='222' text-anchor='middle' fill='{MUTED}' font-size='11'>written in order, never duplicated</text>")
y = 276
for x, label, val in [(30, "same ring", "0.1 µs"), (250, "mirror on the same host", "3.8 µs"),
                      (490, "mirror across a 1 GbE LAN", "30 µs"), (730, "Tokyo → Los Angeles", "51.7 ms, p99 +50 µs")]:
    s.append(f"<text x='{x}' y='{y}' fill='{INK}' font-weight='bold' font-size='13'>{esc(val)}</text>")
    s.append(f"<text x='{x}' y='{y + 16}' fill='{MUTED}' font-size='11'>{esc(label)}</text>")
s.append(f"<text x='30' y='{y + 40}' fill='{MUTED}' font-size='11'>push → read, p50, measured on each host; remote hosts corrected for clock offset</text>")
write("mirror-pipeline.svg", s)

"####;
    std::fs::write(&file, source).unwrap();
    let config = GenericLspConfig::for_python();
    assert!(config.command.contains("pyright"), "real pyright required");
    let engine = GenericLspEngine::spawn(&root, config).await.unwrap();
    let uri = url::Url::from_file_path(&file).unwrap().to_string();
    let typed = source.replace(
        "def write(name, s):",
        "def write(name: str, s: list[str]) -> None:",
    );
    for (i, text) in [source, source, typed.as_str()].into_iter().enumerate() {
        engine.send_notification("textDocument/didOpen",serde_json::json!({"textDocument":{"uri":uri,"languageId":"python","version":1,"text":text}})).await.unwrap();
        let items = engine
            .current_diagnostics_for(&uri, Duration::from_secs(30))
            .await
            .unwrap();
        let identities: Vec<_> = items
            .iter()
            .filter(|x| {
                x["message"]
                    .as_str()
                    .is_some_and(|m| m.contains("builtins.str") && m.contains("not assignable"))
            })
            .collect();
        eprintln!("direct step {i}: identity errors {}", identities.len());
        assert!(
            identities.is_empty(),
            "direct identity failure: {identities:?}"
        );
        engine
            .send_notification(
                "textDocument/didClose",
                serde_json::json!({"textDocument":{"uri":uri}}),
            )
            .await
            .unwrap();
    }
}

/// Pyright-family servers keep one document identity for the engine's lifetime: closing and
/// reopening can split builtin identities (#466). The adapter restores disk text on close and
/// changes that retained document for the next session, with exact monotonic versions.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retained_documents_restore_disk_text_and_reopen_as_changes() {
    let (dir, script) = workspace();
    let file = dir.path().join("retained.py");
    std::fs::write(&file, "baseline\n").unwrap();
    let mut settings = config(&script);
    settings.retain_open_documents = true;
    settings
        .env
        .insert("FAKE_ECHO_CHANGE".to_string(), "1".to_string());
    let engine = GenericLspEngine::spawn(dir.path(), settings).await.unwrap();
    let uri = url::Url::from_file_path(&file).unwrap().to_string();

    engine
        .send_session_notification(
            7,
            "textDocument/didOpen",
            serde_json::json!({"textDocument":{"uri":uri,"languageId":"python","version":1,"text":"proposal one\n"}}),
        )
        .await
        .unwrap();
    engine.close_session(7).await.unwrap();
    let restored = engine
        .current_diagnostics_for(&uri, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(restored[0]["message"], "baseline\n");

    engine
        .send_session_notification(
            8,
            "textDocument/didOpen",
            serde_json::json!({"textDocument":{"uri":uri,"languageId":"python","version":1,"text":"proposal two\n"}}),
        )
        .await
        .unwrap();
    let reopened = engine
        .current_diagnostics_for(&uri, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(reopened[0]["message"], "proposal two\n");
    engine.close_session(8).await.unwrap();

    assert_eq!(std::fs::read_to_string(file).unwrap(), "baseline\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn primary_review_closing_one_session_preserves_the_other_session_text() {
    let (dir, script) = workspace();
    let file = dir.path().join("retained.py");
    std::fs::write(&file, "baseline\n").unwrap();
    let mut settings = config(&script);
    settings.retain_open_documents = true;
    settings.env.insert("FAKE_ECHO_CHANGE".into(), "1".into());
    let engine = GenericLspEngine::spawn(dir.path(), settings).await.unwrap();
    let uri = url::Url::from_file_path(&file).unwrap().to_string();
    for (session, text) in [(7, "proposal one\n"), (8, "proposal two\n")] {
        engine.send_session_notification(session,"textDocument/didOpen",serde_json::json!({"textDocument":{"uri":uri,"languageId":"python","version":1,"text":text}})).await.unwrap();
    }
    assert_eq!(
        engine
            .current_diagnostics_for(&uri, Duration::from_secs(5))
            .await
            .unwrap()[0]["message"],
        "proposal two\n"
    );
    engine.close_session(7).await.unwrap();
    let current = engine
        .current_diagnostics_for(&uri, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(
        current[0]["message"], "proposal two\n",
        "closing another client discarded the active overlay"
    );
    engine.close_session(8).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn direct_and_session_owners_close_without_discarding_each_other() {
    let (dir, script) = workspace();
    let file = dir.path().join("owned.py");
    std::fs::write(&file, "baseline\n").unwrap();
    let mut settings = config(&script);
    settings.retain_open_documents = true;
    settings.env.insert("FAKE_ECHO_CHANGE".into(), "1".into());
    let engine = GenericLspEngine::spawn(dir.path(), settings).await.unwrap();
    let uri = url::Url::from_file_path(&file).unwrap().to_string();
    engine
        .send_session_notification(7, "textDocument/didOpen", serde_json::json!({
            "textDocument": {"uri": uri, "languageId": "python", "version": 1, "text": "session\n"}
        }))
        .await
        .unwrap();
    engine
        .send_notification("textDocument/didOpen", serde_json::json!({
            "textDocument": {"uri": uri, "languageId": "python", "version": 1, "text": "direct\n"}
        }))
        .await
        .unwrap();
    engine.close_session(7).await.unwrap();
    assert_eq!(
        engine
            .current_diagnostics_for(&uri, Duration::from_secs(5))
            .await
            .unwrap()[0]["message"],
        "direct\n"
    );
    engine
        .send_notification(
            "textDocument/didClose",
            serde_json::json!({"textDocument": {"uri": uri}}),
        )
        .await
        .unwrap();
    assert_eq!(
        engine
            .current_diagnostics_for(&uri, Duration::from_secs(5))
            .await
            .unwrap()[0]["message"],
        "baseline\n"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn watched_disk_changes_refresh_only_logically_closed_retained_documents() {
    let (dir, script) = workspace();
    let closed = dir.path().join("closed.py");
    let active = dir.path().join("active.py");
    std::fs::write(&closed, "closed baseline\n").unwrap();
    std::fs::write(&active, "active baseline\n").unwrap();
    let mut settings = config(&script);
    settings.retain_open_documents = true;
    settings.env.insert("FAKE_ECHO_CHANGE".into(), "1".into());
    let engine = GenericLspEngine::spawn(dir.path(), settings).await.unwrap();
    let closed_uri = url::Url::from_file_path(&closed).unwrap().to_string();
    let active_uri = url::Url::from_file_path(&active).unwrap().to_string();
    for (uri, text) in [
        (&closed_uri, "closed overlay\n"),
        (&active_uri, "active overlay\n"),
    ] {
        engine
            .send_notification(
                "textDocument/didOpen",
                serde_json::json!({
                    "textDocument": {"uri": uri, "languageId": "python", "version": 1, "text": text}
                }),
            )
            .await
            .unwrap();
    }
    engine
        .send_notification(
            "textDocument/didChange",
            serde_json::json!({
                "textDocument": {"uri": active_uri, "version": 2},
                "contentChanges": [{"text": "active overlay\n"}]
            }),
        )
        .await
        .unwrap();
    engine
        .send_notification(
            "textDocument/didClose",
            serde_json::json!({
                "textDocument": {"uri": closed_uri}
            }),
        )
        .await
        .unwrap();
    std::fs::write(&closed, "closed changed\n").unwrap();
    std::fs::write(&active, "active changed\n").unwrap();
    engine
        .send_notification(
            "workspace/didChangeWatchedFiles",
            serde_json::json!({
                "changes": [
                    {"uri": closed_uri, "type": 2},
                    {"uri": active_uri, "type": 2}
                ]
            }),
        )
        .await
        .unwrap();
    assert_eq!(
        engine
            .current_diagnostics_for(&closed_uri, Duration::from_secs(5))
            .await
            .unwrap()[0]["message"],
        "closed changed\n"
    );
    assert_eq!(
        engine
            .current_diagnostics_for(&active_uri, Duration::from_secs(5))
            .await
            .unwrap()[0]["message"],
        "active overlay\n"
    );
    std::fs::remove_file(&closed).unwrap();
    engine
        .send_notification(
            "workspace/didChangeWatchedFiles",
            serde_json::json!({"changes": [{"uri": closed_uri, "type": 3}]}),
        )
        .await
        .unwrap();
    assert!(
        !engine.accepts_documents(),
        "a truly closed retained identity retires the whole generation"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_full_retained_generation_requires_whole_engine_eviction() {
    let (dir, script) = workspace();
    let file = dir.path().join("bounded.py");
    std::fs::write(&file, "baseline\n").unwrap();
    let mut settings = config(&script);
    settings.retain_open_documents = true;
    settings.max_retained_documents = 1;
    settings.env.insert("FAKE_ECHO_CHANGE".into(), "1".into());
    let engine = GenericLspEngine::spawn(dir.path(), settings).await.unwrap();
    let uri = url::Url::from_file_path(&file).unwrap().to_string();
    engine.send_notification("textDocument/didOpen", serde_json::json!({
        "textDocument": {"uri": uri, "languageId": "python", "version": 1, "text": "proposal\n"}
    })).await.unwrap();
    engine
        .send_notification(
            "textDocument/didClose",
            serde_json::json!({
                "textDocument": {"uri": uri}
            }),
        )
        .await
        .unwrap();
    assert!(
        !engine.accepts_documents(),
        "the full generation is retired"
    );
    let err = engine.send_notification("textDocument/didOpen", serde_json::json!({
        "textDocument": {"uri": uri, "languageId": "python", "version": 1, "text": "another\n"}
    })).await.expect_err("an evicted identity must not be reopened in the same server");
    assert!(format!("{err:#}").contains("restart"), "{err:#}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn document_version_overflow_is_refused_without_losing_ownership() {
    let (dir, script) = workspace();
    let file = dir.path().join("overflow.py");
    std::fs::write(&file, "baseline\n").unwrap();
    let mut settings = config(&script);
    settings.retain_open_documents = true;
    let engine = GenericLspEngine::spawn(dir.path(), settings).await.unwrap();
    let uri = url::Url::from_file_path(&file).unwrap().to_string();
    engine.send_session_notification(9, "textDocument/didOpen", serde_json::json!({
        "textDocument": {"uri": uri, "languageId": "python", "version": i64::MAX, "text": "proposal\n"}
    })).await.unwrap();
    let err = engine
        .close_session(9)
        .await
        .expect_err("version overflow is explicit");
    assert!(format!("{err:#}").contains("version overflowed"), "{err:#}");
    let again = engine
        .close_session(9)
        .await
        .expect_err("ownership remains for retry");
    assert!(
        format!("{again:#}").contains("version overflowed"),
        "{again:#}"
    );
}

#[tokio::test]
#[ignore = "requires basedpyright"]
async fn primary_review_closed_python_document_follows_later_disk_changes() {
    for retain in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let file = root.join("value.py");
        let good = "def answer():\n    return 1\n";
        std::fs::write(&file, good).unwrap();
        let mut settings = GenericLspConfig::for_python();
        settings.retain_open_documents = retain;
        let engine = GenericLspEngine::spawn(&root, settings).await.unwrap();
        let uri = url::Url::from_file_path(&file).unwrap().to_string();
        engine.send_notification("textDocument/didOpen",serde_json::json!({"textDocument":{"uri":uri,"languageId":"python","version":1,"text":good}})).await.unwrap();
        assert!(
            !engine
                .current_diagnostics_for(&uri, Duration::from_secs(30))
                .await
                .unwrap()
                .iter()
                .any(|d| d["severity"] == 1)
        );
        let consumer = root.join("consumer.py");
        let consumer_text = "import value\nnumber: int = value.answer()\n";
        std::fs::write(&consumer, consumer_text).unwrap();
        let consumer_uri = url::Url::from_file_path(&consumer).unwrap().to_string();
        engine.send_notification("textDocument/didOpen",serde_json::json!({"textDocument":{"uri":consumer_uri,"languageId":"python","version":1,"text":consumer_text}})).await.unwrap();
        assert!(
            !engine
                .current_diagnostics_for(&consumer_uri, Duration::from_secs(30))
                .await
                .unwrap()
                .iter()
                .any(|d| d["severity"] == 1)
        );
        engine
            .send_notification(
                "textDocument/didClose",
                serde_json::json!({"textDocument":{"uri":uri}}),
            )
            .await
            .unwrap();
        std::fs::write(&file, "def answer():\n    return \"bad\"\n").unwrap();
        engine
            .send_notification(
                "workspace/didChangeWatchedFiles",
                serde_json::json!({"changes":[{"uri":uri,"type":2}]}),
            )
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_secs(2)).await;
        let report = engine
            .current_diagnostics_for(&consumer_uri, Duration::from_secs(30))
            .await
            .unwrap();
        assert!(
            report.iter().any(|d| d["severity"] == 1),
            "retain={retain}: disk type error disappeared: {report:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn primary_review_incremental_owner_is_restored_as_its_complete_text() {
    let (dir, script) = workspace();
    let file = dir.path().join("incremental.py");
    std::fs::write(&file, "baseline\n").unwrap();
    let mut settings = config(&script);
    settings.retain_open_documents = true;
    settings.env.insert("FAKE_ECHO_CHANGE".into(), "1".into());
    let engine = GenericLspEngine::spawn(dir.path(), settings).await.unwrap();
    let uri = url::Url::from_file_path(&file).unwrap().to_string();
    engine.send_session_notification(7,"textDocument/didOpen",serde_json::json!({"textDocument":{"uri":uri,"languageId":"python","version":1,"text":"abc\n"}})).await.unwrap();
    engine.send_session_notification(7,"textDocument/didChange",serde_json::json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[{"range":{"start":{"line":0,"character":1},"end":{"line":0,"character":2}},"text":"x"}]})).await.unwrap();
    engine.send_session_notification(8,"textDocument/didOpen",serde_json::json!({"textDocument":{"uri":uri,"languageId":"python","version":1,"text":"other proposal\n"}})).await.unwrap();
    engine.close_session(8).await.unwrap();
    let current = engine
        .current_diagnostics_for(&uri, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(
        current[0]["message"], "axc\n",
        "restored an incremental fragment instead of the remaining owner's full text"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn primary_review_retained_bound_survives_a_long_lived_active_owner() {
    let (dir, script) = workspace();
    let mut settings = config(&script);
    settings.retain_open_documents = true;
    settings.max_retained_documents = 2;
    let engine = GenericLspEngine::spawn(dir.path(), settings).await.unwrap();
    let anchor = dir.path().join("anchor.py");
    std::fs::write(&anchor, "anchor\n").unwrap();
    let anchor_uri = url::Url::from_file_path(&anchor).unwrap().to_string();
    engine.send_session_notification(7,"textDocument/didOpen",serde_json::json!({"textDocument":{"uri":anchor_uri,"languageId":"python","version":1,"text":"active editor\n"}})).await.unwrap();
    for i in 0..2 {
        let file = dir.path().join(format!("closed{i}.py"));
        std::fs::write(&file, "disk\n").unwrap();
        let uri = url::Url::from_file_path(&file).unwrap().to_string();
        engine.send_session_notification(8,"textDocument/didOpen",serde_json::json!({"textDocument":{"uri":uri,"languageId":"python","version":1,"text":"proposal\n"}})).await.unwrap();
        engine.close_session(8).await.unwrap();
    }
    assert!(
        !engine.accepts_documents(),
        "an active owner must not disable the retained-document limit indefinitely"
    );
    engine.send_session_notification(7,"textDocument/didChange",serde_json::json!({"textDocument":{"uri":anchor_uri,"version":2},"contentChanges":[{"text":"still active\n"}]})).await.expect("existing owners may keep editing the retired generation");
    engine.close_session(7).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn primary_review_ordered_utf16_changes_use_their_owners_text() {
    for retain in [false, true] {
        let (dir, script) = workspace();
        let file = dir.path().join("unicode.py");
        std::fs::write(&file, "disk\n").unwrap();
        let mut settings = config(&script);
        settings.retain_open_documents = retain;
        settings.env.insert("FAKE_ECHO_CHANGE".into(), "1".into());
        let engine = GenericLspEngine::spawn(dir.path(), settings).await.unwrap();
        let uri = url::Url::from_file_path(&file).unwrap().to_string();
        engine.send_session_notification(7,"textDocument/didOpen",serde_json::json!({"textDocument":{"uri":uri,"version":1,"languageId":"python","text":"a😀b\r\nsecond\n"}})).await.unwrap();
        engine.send_session_notification(8,"textDocument/didOpen",serde_json::json!({"textDocument":{"uri":uri,"version":1,"languageId":"python","text":"another owner\n"}})).await.unwrap();
        engine.send_session_notification(7,"textDocument/didChange",serde_json::json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[
            {"range":{"start":{"line":0,"character":1},"end":{"line":0,"character":3}},"text":"x"},
            {"range":{"start":{"line":1,"character":0},"end":{"line":1,"character":6}},"text":"tail"}
        ]})).await.unwrap();
        assert_eq!(
            engine
                .current_diagnostics_for(&uri, Duration::from_secs(5))
                .await
                .unwrap()[0]["message"],
            "axb\r\ntail\n"
        );
        engine.close_session(7).await.unwrap();
        assert_eq!(
            engine
                .current_diagnostics_for(&uri, Duration::from_secs(5))
                .await
                .unwrap()[0]["message"],
            "another owner\n"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn primary_review_invalid_incremental_ranges_preserve_the_whole_owner_state() {
    let (dir, script) = workspace();
    let file = dir.path().join("invalid-range.py");
    std::fs::write(&file, "disk\n").unwrap();
    let mut settings = config(&script);
    settings.retain_open_documents = true;
    settings.env.insert("FAKE_ECHO_CHANGE".into(), "1".into());
    let engine = GenericLspEngine::spawn(dir.path(), settings).await.unwrap();
    let uri = url::Url::from_file_path(&file).unwrap().to_string();
    let original = "a😀b\r\nlast\r";
    engine.send_session_notification(7,"textDocument/didOpen",serde_json::json!({"textDocument":{"uri":uri,"version":1,"languageId":"python","text":original}})).await.unwrap();
    for bad_range in [
        serde_json::json!({"start":{"line":0,"character":2},"end":{"line":0,"character":3}}),
        serde_json::json!({"start":{"line":0,"character":3},"end":{"line":0,"character":1}}),
        serde_json::json!({"start":{"line":8,"character":0},"end":{"line":8,"character":0}}),
        serde_json::json!({"start":{"line":0,"character":5},"end":{"line":0,"character":5}}),
        serde_json::json!({"start":{"line":-1,"character":0},"end":{"line":0,"character":0}}),
        serde_json::json!({"start":{"line":0,"character":0.5},"end":{"line":0,"character":1}}),
        serde_json::Value::Null,
    ] {
        let error = engine.send_session_notification(7,"textDocument/didChange",serde_json::json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[
            {"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},"text":"z"},
            {"range":bad_range,"text":"wrong"}
        ]})).await.expect_err("invalid second edit is refused before any owner state change");
        assert!(
            format!("{error:#}").contains("incremental change"),
            "{error:#}"
        );
        engine.send_session_notification(8,"textDocument/didOpen",serde_json::json!({"textDocument":{"uri":uri,"version":1,"languageId":"python","text":"temporary\n"}})).await.unwrap();
        engine.close_session(8).await.unwrap();
        assert_eq!(
            engine
                .current_diagnostics_for(&uri, Duration::from_secs(5))
                .await
                .unwrap()[0]["message"],
            original
        );
    }
    // A complete reset followed by a range edit uses the newly reset text, in order.
    engine.send_session_notification(7,"textDocument/didChange",serde_json::json!({"textDocument":{"uri":uri,"version":3},"contentChanges":[
        {"text":"fresh\n"}, {"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":5}},"text":"done"}
    ]})).await.unwrap();
    assert_eq!(
        engine
            .current_diagnostics_for(&uri, Duration::from_secs(5))
            .await
            .unwrap()[0]["message"],
        "done\n"
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notification_deadline_covers_a_blocked_frame() {
    let (dir, script) = workspace();
    let mut settings = config(&script);
    settings
        .env
        .insert("FAKE_STOP_READING".to_string(), "1".to_string());
    settings.request_timeout = Duration::from_millis(200);
    let pid_file = dir.path().join("pid");
    settings.env.insert(
        "FAKE_PID_FILE".to_string(),
        pid_file.to_string_lossy().into_owned(),
    );
    let engine = GenericLspEngine::spawn(dir.path(), settings).await.unwrap();
    let outcome = tokio::time::timeout(
        Duration::from_secs(2),
        engine.send_notification("textDocument/didOpen", serde_json::json!({
            "textDocument": {"uri": "file:///notification.txt", "languageId":"text", "version":1, "text": "x".repeat(8 * 1024 * 1024)}
        })),
    ).await.expect("notification must honor its internal write budget");
    let error = outcome.expect_err("the server never reads the notification");
    let text = format!("{error:#}");
    assert!(text.contains("textDocument/didOpen"), "{text}");
    assert!(text.to_lowercase().contains("timeout"), "{text}");
    assert!(
        !engine.is_alive(),
        "a partial document frame cannot be reused"
    );
    assert!(
        !engine.accepts_documents(),
        "state changed before the failed write invalidates the generation"
    );
    assert_process_exits(&pid_file).await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_a_notification_retires_its_owned_process() {
    let (dir, script) = workspace();
    let mut settings = config(&script);
    settings
        .env
        .insert("FAKE_STOP_READING".to_string(), "1".to_string());
    settings.request_timeout = Duration::from_secs(10);
    let pid_file = dir.path().join("pid");
    settings.env.insert(
        "FAKE_PID_FILE".to_string(),
        pid_file.to_string_lossy().into_owned(),
    );
    let engine = std::sync::Arc::new(GenericLspEngine::spawn(dir.path(), settings).await.unwrap());
    let writing = {
        let engine = std::sync::Arc::clone(&engine);
        tokio::spawn(async move {
            engine
                .send_notification(
                    "prodCode/cancelledNotification",
                    serde_json::json!({"payload": "x".repeat(8 * 1024 * 1024)}),
                )
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    writing.abort();
    assert!(writing.await.expect_err("cancelled").is_cancelled());
    assert!(!engine.is_alive());
    assert_process_exits(&pid_file).await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_automatic_response_retires_the_server() {
    let (dir, script) = workspace();
    let mut settings = config(&script);
    settings
        .env
        .insert("FAKE_HUGE_AUTO_REQUEST".to_string(), "1".to_string());
    settings.request_timeout = Duration::from_millis(200);
    let pid_file = dir.path().join("pid");
    settings.env.insert(
        "FAKE_PID_FILE".to_string(),
        pid_file.to_string_lossy().into_owned(),
    );
    let engine = GenericLspEngine::spawn(dir.path(), settings).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while engine.is_alive() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the automatic response has a bounded write");
    assert_process_exits(&pid_file).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_complete_notification_keeps_document_restore_healthy() {
    let (dir, script) = workspace();
    let file = dir.path().join("healthy.py");
    std::fs::write(&file, "disk\n").unwrap();
    let mut settings = config(&script);
    settings.retain_open_documents = true;
    settings.env.insert("FAKE_ECHO_CHANGE".into(), "1".into());
    let engine = GenericLspEngine::spawn(dir.path(), settings).await.unwrap();
    let uri = url::Url::from_file_path(&file).unwrap().to_string();
    engine
        .send_session_notification(
            7,
            "textDocument/didOpen",
            serde_json::json!({"textDocument":{"uri":uri,"languageId":"python","version":1,"text":"overlay\n"}}),
        )
        .await
        .unwrap();
    engine.close_session(7).await.unwrap();
    assert_eq!(
        engine
            .current_diagnostics_for(&uri, Duration::from_secs(5))
            .await
            .unwrap()[0]["message"],
        "disk\n"
    );
    assert!(engine.is_alive());
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_the_engine_kills_a_reader_owned_server() {
    let (dir, script) = workspace();
    let mut settings = config(&script);
    let pid_file = dir.path().join("pid");
    settings.env.insert(
        "FAKE_PID_FILE".to_string(),
        pid_file.to_string_lossy().into_owned(),
    );
    let engine = GenericLspEngine::spawn(dir.path(), settings).await.unwrap();
    drop(engine);
    assert_process_exits(&pid_file).await;
}

#[tokio::test]
async fn a_content_type_header_after_length_keeps_frames_aligned() {
    let (dir, script) = workspace();
    let mut settings = config(&script);
    settings.request_timeout = Duration::from_millis(200);
    settings.env.insert("FAKE_CONTENT_TYPE".into(), "1".into());
    let engine = GenericLspEngine::spawn(dir.path(), settings)
        .await
        .expect("the optional Content-Type header must not break initialization");
    let answer = engine
        .send_request("textDocument/hover", serde_json::json!({}))
        .await
        .expect("successive messages must stay aligned");
    assert_eq!(
        answer
            .pointer("/result/contents/value")
            .and_then(|v| v.as_str()),
        Some("the fake server answered")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn malformed_frames_retire_the_generic_server_and_wake_pending_requests() {
    for kind in ["duplicate", "oversize", "header", "truncated"] {
        let (dir, script) = workspace();
        let pid_file = dir.path().join("pid");
        let mut settings = config(&script);
        settings.request_timeout = Duration::from_secs(30);
        settings.env.insert(
            "FAKE_PID_FILE".into(),
            pid_file.to_string_lossy().into_owned(),
        );
        let engine = GenericLspEngine::spawn(dir.path(), settings).await.unwrap();
        let error = tokio::time::timeout(
            Duration::from_secs(2),
            engine.send_request("prodCode/brokenFrame", serde_json::json!({"kind":kind})),
        )
        .await
        .expect("bad frames must wake requests before their request deadline")
        .expect_err("malformed input must not become a response");
        assert!(
            format!("{error:#}").contains("prodCode/brokenFrame"),
            "{error:#}"
        );
        assert!(!engine.is_alive());
        assert_process_exits(&pid_file).await;
    }
}
