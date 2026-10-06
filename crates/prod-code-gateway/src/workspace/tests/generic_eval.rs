/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::sync::Arc;
use std::time::Duration;

use super::fixtures::generic_workspace;
use crate::workspace::probes::{SWIFT_PROBE, swift_probe_file, with_probe_line};
use crate::workspace::shared::SharedWorkspace;

#[tokio::test]
async fn generic_validation_capacity_failure_never_falls_back_to_the_main_engine() {
    let dir = tempfile::tempdir().unwrap();
    let (workspace, _) = generic_workspace(dir.path()).await;
    let admission = Arc::new(crate::admission::Admission::with_probe(
        crate::admission::scripted_probe(vec![(10, 100)]),
        2048,
        Duration::ZERO,
    ));
    let err = match workspace.validation_view(&admission).await {
        Ok(_) => panic!("capacity refusal must not return the main engine"),
        Err(err) => err,
    };
    assert!(format!("{err:#}").starts_with("capacity: "), "{err:#}");
    assert!(
        workspace
            .generic_engine
            .as_ref()
            .unwrap()
            .accepts_documents()
    );
}

#[tokio::test]
async fn generic_validation_start_failure_never_falls_back_to_the_main_engine() {
    let dir = tempfile::tempdir().unwrap();
    let (workspace, script) = generic_workspace(dir.path()).await;
    std::fs::remove_file(script).unwrap();
    let err = match workspace
        .validation_view(&Arc::new(crate::admission::Admission::unbounded()))
        .await
    {
        Ok(_) => panic!("start failure must not return the main engine"),
        Err(err) => err,
    };
    assert!(
        format!("{err:#}").contains("private python validation server failed to start"),
        "{err:#}"
    );
    assert!(workspace.generic_engine.as_ref().unwrap().is_alive());
}

/// A valid initialized server remains reusable until its output actually exits.
#[tokio::test]
async fn a_workspace_whose_server_exited_is_loaded_afresh() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = dir.path().join("server.py");
    std::fs::write(&script, r#"import json, sys
while True:
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line: sys.exit(0)
        if line == b"\r\n": break
        if line.lower().startswith(b"content-length:"):
            length = int(line.split(b":", 1)[1])
    message = json.loads(sys.stdin.buffer.read(length))
    if message.get("method") == "initialize":
        body = json.dumps({"jsonrpc":"2.0", "id":message["id"], "result":{"capabilities":{}}}).encode()
        sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
        sys.stdout.buffer.flush()
    elif message.get("method") == "initialized" and len(sys.argv) > 1:
        sys.exit(0)
"#).expect("fake server script");
    let workspace = |engine: prod_code_engine_generic::GenericLspEngine| {
        SharedWorkspace::new(
            dir.path().to_path_buf(),
            "typescript".to_string(),
            None,
            None,
            Some(Arc::new(engine)),
            None,
        )
    };
    let config = |exit: bool| {
        let mut args = vec![script.to_string_lossy().into_owned()];
        if exit {
            args.push("exit-after-initialized".to_string());
        }
        prod_code_engine_generic::GenericLspConfig {
            command: "python3".to_string(),
            args,
            ..Default::default()
        }
    };
    let running = workspace(
        prod_code_engine_generic::GenericLspEngine::spawn(dir.path(), config(false))
            .await
            .expect("valid server initializes"),
    );
    let exited = workspace(
        prod_code_engine_generic::GenericLspEngine::spawn(dir.path(), config(true))
            .await
            .expect("short-lived valid server initializes"),
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !exited.has_dead_server() && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(exited.has_dead_server(), "a server that exited is noticed");
    assert!(
        !exited.reusable_for("typescript"),
        "and its workspace is loaded afresh"
    );
    assert!(!running.has_dead_server(), "a running server is not dead");
    assert!(
        running.reusable_for("typescript"),
        "and its workspace is reused"
    );
    assert!(
        !running.reusable_for("rust"),
        "unless another engine is asked for"
    );
}

#[test]
fn a_swift_probe_is_a_package_source_with_a_type_error_on_its_own_last_line() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    assert!(
        swift_probe_file(root).is_none(),
        "no Package.swift, nothing to wait for"
    );
    std::fs::write(root.join("Package.swift"), "// swift-tools-version:5.9\n").unwrap();
    assert!(swift_probe_file(root).is_none(), "no sources");
    for (path, text) in [
        (
            ".build/checkouts/Dep/Sources/Dep/a.swift",
            "// a dependency\n",
        ),
        ("Sources/Shop/main.swift", "print(1)\n"),
        ("Sources/Shop/Pricing.swift", "func price() -> Int { 1 }"),
        ("Sources/Shop/notes.txt", "not swift\n"),
    ] {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
    }
    let (path, text) = swift_probe_file(root).expect("a source");
    assert!(path.ends_with("Sources/Shop/Pricing.swift"), "{path:?}");
    let (probe, line) = with_probe_line(&text);
    assert_eq!(
        probe,
        "func price() -> Int { 1 }\nlet __prodCodeProbe: Int = \"\"\n"
    );
    assert_eq!(line, 1);
    let (probe, line) = with_probe_line("a\nb\n");
    assert_eq!(probe.lines().nth(line as usize), Some(SWIFT_PROBE));
    assert_eq!(with_probe_line("").1, 0);
}
