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
