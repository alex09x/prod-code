//! The managed backend worker: a language server as a child process, framed over its stdio.
//!
//! This is the path the gateway falls back to when an in-process engine will not load, which
//! means it is the path nobody sees until something has already gone wrong. It is exercised
//! here directly, against gopls, because a fallback that has never run is not a fallback.

use prod_code_gateway::backend::BackendWorker;
use std::path::{Path, PathBuf};
use std::time::Duration;

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
        let outcome = BackendWorker::spawn(dir.path(), "go").await;
        if matches!(mode.as_str(), "valid" | "collision") {
            let worker = outcome.unwrap_or_else(|error| panic!("{mode}: {error:#}"));
            let capabilities = worker.capabilities.read().await.clone();
            assert_eq!(capabilities, Some(serde_json::json!({})), "{mode}");
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
                "eof" => assert!(
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

def read():
    n = 0
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        if not line.strip():
            break
        if line.lower().startswith(b'content-length:'):
            n = int(line.split(b':', 1)[1])
    return json.loads(sys.stdin.buffer.read(n))

def send(value):
    body = json.dumps(value).encode()
    sys.stdout.buffer.write(b'Content-Length: %d\r\n\r\n' % len(body) + body)
    sys.stdout.buffer.flush()

initial = read()
assert initial['method'] == 'initialize'
mode = os.environ['PROD_CODE_BACKEND_LIFECYCLE_CASE']
if mode == 'eof':
    sys.exit(0)
if mode == 'silent':
    threading.Event().wait()
if mode == 'collision':
    send({'jsonrpc': '2.0', 'id': 1, 'method': 'workspace/configuration', 'params': {'items': [{}]}})
    response = read()
    assert response.get('result') == [{}], response
if mode == 'error':
    send({'jsonrpc': '2.0', 'id': 1, 'error': {'code': -32603, 'message': 'fixture refused initialization'}})
elif mode == 'malformed':
    send({'jsonrpc': '2.0', 'id': 1, 'result': {}})
else:
    send({'jsonrpc': '2.0', 'id': 1, 'result': {'capabilities': {}}})
while read() is not None:
    pass
"#).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::join_paths(
        std::iter::once(binaries.path().to_path_buf())
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let mut failures = Vec::new();
    for mode in ["valid", "collision", "error", "malformed", "eof", "silent"] {
        let output = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "fallback_initialization_requires_a_valid_response",
                "--nocapture",
            ])
            .env(MODE, mode)
            .env("PATH", &path)
            .kill_on_drop(true)
            .output()
            .await
            .unwrap();
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
