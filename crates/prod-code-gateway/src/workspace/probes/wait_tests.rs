/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The wait for a SwiftPM workspace's build settings, against a scripted language server.

use super::*;
use prod_code_engine_generic::{GenericLspConfig, GenericLspEngine};
use std::collections::HashMap;

/// A language server that records every opened text and answers diagnostic pulls by `FAKE_MODE`:
/// `typed` reports a type error on the last line of the opened text, `clean` reports nothing, and
/// `refuse` does not know the method.
const SERVER: &str = r#"
import json, os, sys

def send(message):
    body = json.dumps(message).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
    sys.stdout.buffer.flush()

def read():
    length = 0
    while True:
        header = sys.stdin.buffer.readline()
        if not header:
            return None
        header = header.strip()
        if not header:
            break
        if header.lower().startswith(b"content-length:"):
            length = int(header.split(b":")[1])
    return json.loads(sys.stdin.buffer.read(length)) if length else None

mode = os.environ["FAKE_MODE"]
lines = {}
while True:
    message = read()
    if message is None:
        break
    method = message.get("method", "")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": message["id"], "result": {"capabilities": {}}})
    elif method == "textDocument/didOpen":
        document = message["params"]["textDocument"]
        lines[document["uri"]] = len(document["text"].splitlines())
        with open(os.environ["FAKE_LOG"], "a") as log:
            log.write(json.dumps([document["uri"], document["languageId"], document["text"]]) + "\n")
    elif method == "textDocument/diagnostic" and mode != "refuse":
        uri = message["params"]["textDocument"]["uri"]
        items = []
        if mode == "typed":
            last = lines.get(uri, 1) - 1
            items = [{"range": {"start": {"line": last, "character": 0}, "end": {"line": last, "character": 1}},
                      "severity": 1, "message": "cannot convert value of type 'String' to specified type 'Int'"}]
        send({"jsonrpc": "2.0", "id": message["id"], "result": {"kind": "full", "items": items}})
    elif "id" in message:
        send({"jsonrpc": "2.0", "id": message["id"], "error": {"code": -32601, "message": "method not found"}})
    elif method == "exit":
        break
"#;

struct Server {
    engine: GenericLspEngine,
    log: PathBuf,
    _dir: tempfile::TempDir,
}

impl Server {
    async fn start(mode: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("server.py");
        std::fs::write(&script, SERVER).unwrap();
        let log = dir.path().join("opened.jsonl");
        let config = GenericLspConfig {
            command: "python3".to_string(),
            args: vec![script.to_string_lossy().into_owned()],
            env: HashMap::from([
                ("FAKE_MODE".to_string(), mode.to_string()),
                ("FAKE_LOG".to_string(), log.to_string_lossy().into_owned()),
            ]),
            ..Default::default()
        };
        let engine = GenericLspEngine::spawn(dir.path(), config).await.unwrap();
        Self {
            engine,
            log,
            _dir: dir,
        }
    }

    /// The `[uri, language, text]` of every document opened so far.
    fn opened(&self) -> Vec<(String, String, String)> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

fn swift_package(root: &Path) {
    std::fs::create_dir_all(root.join("Sources/App")).unwrap();
    std::fs::write(root.join("Package.swift"), "// pkg").unwrap();
    std::fs::write(root.join("Sources/App/a.swift"), "print(1)").unwrap();
}

#[tokio::test]
async fn the_settings_wait_opens_the_probe_and_says_what_the_server_made_of_it() {
    // (server mode, outcome)
    for (mode, expected) in [
        ("typed", Ok(true)),
        ("clean", Ok(false)),
        ("refuse", Err(true)),
    ] {
        let server = Server::start(mode).await;
        let package = tempfile::tempdir().unwrap();
        swift_package(package.path());
        let checked =
            wait_for_swift_settings_within(&server.engine, package.path(), Duration::from_secs(1))
                .await
                .unwrap();
        match expected {
            Ok(typed) => assert_eq!(checked, Ok(typed), "{mode}"),
            Err(_) => {
                let reason = checked.unwrap_err();
                assert!(
                    reason.contains("/Sources/App/a.swift: "),
                    "{mode}: {reason}"
                );
                assert!(
                    reason.contains("the language server published nothing for the text last sent"),
                    "{mode}: {reason}"
                );
            }
        }
        let opened = server.opened();
        assert_eq!(opened.len(), 1, "{mode}");
        let (uri, language, body) = &opened[0];
        assert!(uri.ends_with("/Sources/App/a.swift"), "{uri}");
        assert_eq!(language, "swift");
        assert_eq!(body, &format!("print(1)\n{SWIFT_PROBE}\n"));
    }
}

#[tokio::test]
async fn the_settings_wait_leaves_roots_without_settings_to_wait_for_alone() {
    let server = Server::start("typed").await;
    let base = tempfile::tempdir().unwrap();
    let mut roots = Vec::new();
    for dir in [
        "target",
        "node_modules",
        "vendor",
        "build",
        "dist",
        ".build",
        "Pods",
        "DerivedData",
    ] {
        let root = base.path().join(dir).join("pkg");
        swift_package(&root);
        roots.push(root);
    }
    // An Xcode project has no Package.swift; a package without sources has nothing to open.
    let xcode = base.path().join("xcode");
    std::fs::create_dir_all(xcode.join("Sources")).unwrap();
    std::fs::write(xcode.join("Sources/a.swift"), "print(1)").unwrap();
    let empty = base.path().join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    std::fs::write(empty.join("Package.swift"), "// pkg").unwrap();
    roots.extend([xcode, empty]);

    let started = std::time::Instant::now();
    for root in &roots {
        assert_eq!(
            wait_for_swift_settings_within(&server.engine, root, Duration::from_secs(30)).await,
            None,
            "{}",
            root.display()
        );
        // The public entry, with its 45 second wait: it returns at once for these.
        wait_for_swift_build_settings(&server.engine, root).await;
    }
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(server.opened(), vec![]);
}
