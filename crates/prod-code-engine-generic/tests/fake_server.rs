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

use prod_code_engine_generic::{GenericLspConfig, GenericLspEngine};
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

/// A language server that does only what these tests need.
const SERVER: &str = r#"
import json, sys, threading

def send(message):
    body = json.dumps(message).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body))
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()

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
    return json.loads(sys.stdin.buffer.read(length))

while True:
    message = read()
    if message is None:
        break
    method = message.get("method", "")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": message["id"], "result": {
            "capabilities": {"hoverProvider": True, "diagnosticProvider": {"interFileDependencies": False}}
        }})
    elif method == "textDocument/hover":
        send({"jsonrpc": "2.0", "id": message["id"], "result": {
            "contents": {"kind": "markdown", "value": "the fake server answered"}
        }})
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

/// Writes the server and a workspace for it to serve.
fn workspace() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = dir.path().join("server.py");
    std::fs::write(&script, SERVER).expect("write the server");
    std::fs::write(dir.path().join("a.txt"), "hello\n").expect("a file to open");
    (dir, script)
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
    let items = engine.diagnostics_for(&uri).await;
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
            .is_empty(),
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
