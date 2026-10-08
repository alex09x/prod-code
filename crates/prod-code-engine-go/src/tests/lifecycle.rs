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
use tempfile::tempdir;

use crate::config::GoConfig;
use crate::engine::GoEngine;
use crate::types::lock_unpoisoned;

use super::fake_server::gopls_is_available_for;
#[cfg(unix)]
use super::fake_server::{FAKE_GOPLS, assert_process_exits, fake_engine};

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn empty_capabilities_are_valid_but_bad_initialize_retires_its_owned_gopls() {
    let (dir, engine) = fake_engine(
        Some(("FAKE_EMPTY_CAPABILITIES", "1")),
        Duration::from_secs(2),
    )
    .await;
    assert_eq!(
        *engine.capabilities.read().await,
        Some(serde_json::json!({}))
    );
    engine
        .send_request("textDocument/hover", serde_json::json!({}))
        .await
        .expect("an empty capability object still initializes a working server");
    drop(engine);
    assert_process_exits(&dir.path().join("pid")).await;

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
        let response = response.to_string();
        let (dir, engine) = fake_engine(
            Some(("FAKE_INITIALIZE_AFTER_FIRST", &response)),
            Duration::from_secs(2),
        )
        .await;
        let waiting = {
            let engine = Arc::clone(&engine);
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
        assert!(engine.capabilities.read().await.is_none());
        assert!(lock_unpoisoned(&engine.pending_requests).is_empty());
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
        let seen = std::fs::read_to_string(dir.path().join("seen")).expect("request log");
        assert_eq!(
            seen.lines()
                .filter(|method| *method == "initialized")
                .count(),
            1,
            "{seen}"
        );
        assert_process_exits(&dir.path().join("pid")).await;
    }
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn initialize_timeout_retires_the_retained_owned_gopls() {
    let (dir, engine) = fake_engine(
        Some(("FAKE_INITIALIZE_AFTER_FIRST", "silence")),
        Duration::from_millis(150),
    )
    .await;
    let error = engine
        .initialize()
        .await
        .expect_err("the retained handshake times out");
    assert!(format!("{error:#}").contains("initialize"), "{error:#}");
    assert!(!engine.is_alive(), "the timed out generation is retired");
    assert!(engine.capabilities.read().await.is_none());
    assert!(lock_unpoisoned(&engine.pending_requests).is_empty());
    assert_process_exits(&dir.path().join("pid")).await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_initialize_retires_the_retained_owned_gopls() {
    let (dir, engine) = fake_engine(
        Some(("FAKE_INITIALIZE_AFTER_FIRST", "delayed")),
        Duration::from_secs(10),
    )
    .await;
    let seen_file = dir.path().join("seen");
    let initializing = {
        let engine = Arc::clone(&engine);
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
    .expect("the retained initialize request reaches gopls");
    let writer = engine.stdin.lock().await;
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(
        !initializing.is_finished(),
        "the handshake is waiting to write initialized"
    );
    initializing.abort();
    assert!(initializing.await.expect_err("cancelled").is_cancelled());
    drop(writer);
    assert!(!engine.is_alive(), "the cancelled generation is retired");
    assert!(engine.capabilities.read().await.is_none());
    assert!(lock_unpoisoned(&engine.pending_requests).is_empty());
    assert_process_exits(&dir.path().join("pid")).await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_initialize_during_first_load_retires_its_owned_gopls() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempdir().expect("tempdir");
    let script = dir.path().join("fake-gopls");
    std::fs::write(&script, FAKE_GOPLS).expect("write fake gopls");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
        .expect("make fake gopls executable");
    let pid_file = dir.path().join("pid");
    let mut config = GoConfig {
        gopls_path: Some(script),
        shared_cache_dir: Some(dir.path().join("cache")),
        ..Default::default()
    };
    config.extra_env.insert(
        "FAKE_INITIALIZE_RESPONSE".into(),
        serde_json::json!({"result": {}}).to_string(),
    );
    config.extra_env.insert(
        "FAKE_PID_FILE".into(),
        pid_file.to_string_lossy().into_owned(),
    );
    let error =
        match GoEngine::load_with_request_timeout(dir.path(), config, Duration::from_secs(2)).await
        {
            Ok(_) => panic!("a malformed first handshake must be refused"),
            Err(error) => error,
        };
    assert!(format!("{error:#}").contains("capabilities"), "{error:#}");
    assert_process_exits(&pid_file).await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_the_engine_kills_a_reader_owned_server() {
    let (dir, engine) = fake_engine(None, Duration::from_secs(2)).await;
    drop(engine);
    assert_process_exits(&dir.path().join("pid")).await;
}

#[cfg(unix)]
#[tokio::test]
async fn malformed_frames_retire_gopls_and_wake_pending_requests() {
    for kind in ["duplicate", "oversize", "header", "truncated"] {
        let (dir, engine) = fake_engine(None, Duration::from_secs(30)).await;
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
        assert_process_exits(&dir.path().join("pid")).await;
    }
}

#[tokio::test]
async fn test_go_engine_lifecycle_if_available() {
    if !gopls_is_available_for("test_go_engine_lifecycle_if_available") {
        return;
    }

    let dir = tempdir().unwrap();
    let go_mod = "module example.com/demo\n\ngo 1.22\n";
    let main_go = r#"package main

import "fmt"

func Greet(name string) string {
	return fmt.Sprintf("Hello, %s!", name)
}

func main() {
	msg := Greet("World")
	fmt.Println(msg)
}
"#;
    std::fs::write(dir.path().join("go.mod"), go_mod).unwrap();
    std::fs::write(dir.path().join("main.go"), main_go).unwrap();

    let config = GoConfig {
        health_probe_interval: Some(Duration::from_millis(100)),
        ..Default::default()
    };
    let engine = GoEngine::load(dir.path(), config).await.unwrap();
    #[cfg(unix)]
    let native_pid_file = {
        let native_pid = lock_unpoisoned(&engine._child)
            .id()
            .expect("owned native gopls PID");
        let path = dir.path().join("native-gopls.pid");
        std::fs::write(&path, native_pid.to_string()).unwrap();
        path
    };

    let file_path = dir.path().join("main.go");
    let file_uri = format!("file://{}", file_path.to_string_lossy());

    // 1. didOpen
    engine.did_open(&file_uri, main_go).await.unwrap();

    // 2. Document symbols
    let symbols = engine.document_symbols(&file_uri).await.unwrap();
    assert!(symbols.is_array());
    let sym_arr = symbols.as_array().unwrap();
    assert!(!sym_arr.is_empty(), "Expected symbols in main.go");

    // 3. Hover on 'Greet'
    let hover = engine.hover(&file_uri, 4, 6).await.unwrap();
    assert!(hover.is_some(), "Expected hover documentation for Greet");

    tokio::time::timeout(Duration::from_secs(30), async {
        while engine.busy().is_some() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("gopls becomes idle before dispatch probes");
    tokio::time::timeout(Duration::from_secs(30), async {
        while engine.health_probe_completions() < 2 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("gopls answers at least two scheduled probes");
    let hover_after_probes = engine.hover(&file_uri, 4, 6).await.unwrap();
    assert!(
        hover_after_probes.is_some(),
        "Expected hover after at least two scheduled dispatch probes"
    );
    assert_eq!(
        std::fs::read_to_string(&file_path).unwrap(),
        main_go,
        "native probing never mutates the fixture source"
    );

    // 4. Definition of 'Greet'
    let def = engine.definition(&file_uri, 9, 8).await.unwrap();
    assert!(def.is_some(), "Expected definition jump for Greet");

    // 5. References of 'Greet'
    let refs = engine.references(&file_uri, 4, 6).await.unwrap();
    assert!(refs.is_array());
    assert!(refs.as_array().unwrap().len() >= 2);
    drop(engine);
    #[cfg(unix)]
    assert_process_exits(&native_pid_file).await;
}

/// The first `workspace/symbol` of a fresh gopls waits for its package loading and finds
/// the function, however soon it is asked: taken as it came, it was empty (#391).
#[tokio::test]
async fn the_first_symbol_search_of_a_fresh_gopls_finds_the_function() {
    if !gopls_is_available_for("the_first_symbol_search_of_a_fresh_gopls_finds_the_function") {
        return;
    }
    // Not `tempdir()`: its `.tmp` name is a directory Go tools skip, and gopls then finds the
    // module's symbols in no package, only in the opened file once it has loaded it.
    let dir = tempfile::Builder::new()
        .prefix("prod-code-go-")
        .tempdir()
        .unwrap();
    std::fs::write(
        dir.path().join("go.mod"),
        "module example.com/subject\n\ngo 1.22\n",
    )
    .unwrap();
    let store = "package subject\n\n// Total sums the quantities.\nfunc Total(all []int) int {\n\tsum := 0\n\tfor _, q := range all {\n\t\tsum += q\n\t}\n\treturn sum\n}\n";
    std::fs::write(dir.path().join("store.go"), store).unwrap();
    let engine = GoEngine::load(dir.path(), GoConfig::default())
        .await
        .unwrap();
    let uri = format!("file://{}", dir.path().join("store.go").to_string_lossy());
    engine.did_open(&uri, store).await.unwrap();
    assert_eq!(engine.open_files.read().await.get(&uri), Some(&1));

    // Verify documentSymbol is answered reliably with index gating
    let doc_symbols = engine
        .send_request(
            "textDocument/documentSymbol",
            serde_json::json!({ "textDocument": { "uri": uri } }),
        )
        .await
        .unwrap();
    let syms = doc_symbols["result"].as_array().map(Vec::len).unwrap_or(0);
    assert!(syms >= 1, "{doc_symbols}");

    // Repeated did_open deduplicates to did_change instead of duplicate didOpen
    engine.did_open(&uri, store).await.unwrap();
    assert_eq!(engine.open_files.read().await.get(&uri), Some(&2));

    let answer = engine
        .send_request("workspace/symbol", serde_json::json!({ "query": "Total" }))
        .await
        .unwrap();
    let hits = answer["result"].as_array().map(Vec::len).unwrap_or(0);
    assert!(hits >= 1, "{answer}");
    assert_eq!(engine.busy(), None);

    engine.did_close(&uri).await.unwrap();
    assert!(!engine.open_files.read().await.contains_key(&uri));
}
