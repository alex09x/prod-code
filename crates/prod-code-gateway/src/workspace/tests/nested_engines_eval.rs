/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! #1488: a change under a checkout reaches the Rust engine of a nested project loaded below
//! it, whose key is `checkout/<subpath>` and not the checkout's.

use crate::workspace::{SharedWorkspace, WorkspaceManager};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A real, small Rust engine for the crate at `root`.
async fn engine_for(root: &Path) -> Arc<tokio::sync::Mutex<prod_code_engine_rust::RustEngine>> {
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"nested\"\nversion = \"0.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn one() -> u8 { 1 }\n").unwrap();
    std::fs::write(
        root.join("prod-code.toml"),
        "[rust]\nproc_macro_srv = \"disabled\"\nbuild_scripts = false\n",
    )
    .unwrap();
    let load = root.to_path_buf();
    let engine =
        tokio::task::spawn_blocking(move || prod_code_engine_rust::RustEngine::load(&load))
            .await
            .unwrap()
            .unwrap();
    Arc::new(tokio::sync::Mutex::new(engine))
}

#[tokio::test]
async fn a_change_in_a_nested_project_reaches_its_engine_once() {
    let dir = tempfile::tempdir().unwrap();
    let checkout = dir.path().join("checkout");
    let nested_root = checkout.join("tools/nested");
    let main = engine_for(&checkout).await;
    let nested = engine_for(&nested_root).await;
    let workspace = |root: &Path, engine: &Arc<_>| {
        Arc::new(SharedWorkspace::new(
            root.to_path_buf(),
            "rust".into(),
            Some(Arc::clone(engine)),
            None,
            None,
            None,
        ))
    };
    let manager = WorkspaceManager::new();
    manager
        .insert_ready_for_test(workspace(&checkout, &main))
        .await;
    manager
        .insert_ready_for_test(workspace(&nested_root, &nested))
        .await;
    // A third workspace sharing the main engine must not make it listed twice.
    manager
        .insert_ready_for_test(workspace(&checkout.join("tools/shared"), &main))
        .await;
    let reached = |changed: Vec<PathBuf>| {
        let manager = &manager;
        let checkout = &checkout;
        async move { manager.rust_engines_for_changes(checkout, &changed).await }
    };
    let is = |engines: &[Arc<_>], engine: &Arc<_>| engines.iter().any(|e| Arc::ptr_eq(e, engine));

    let only_root = reached(vec![checkout.join("src/lib.rs")]).await;
    assert_eq!(only_root.len(), 1, "the checkout's own engine, once");
    assert!(is(&only_root, &main));

    let nested_change = reached(vec![nested_root.join("src/lib.rs")]).await;
    assert_eq!(
        nested_change.len(),
        2,
        "the checkout's and the nested project's"
    );
    assert!(is(&nested_change, &main) && is(&nested_change, &nested));
}

/// #1490: a change made while the validation engine loads, after its loader read the file and
/// before the engine is among those a change reaches, is caught up when it is published.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_change_made_while_the_validation_engine_loads_reaches_it() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("checkout");
    let main = engine_for(&root).await;
    let ws = Arc::new(SharedWorkspace::new(
        root.clone(),
        "rust".into(),
        Some(main),
        None,
        None,
        None,
    ));
    let manager = WorkspaceManager::new();
    manager.insert_ready_for_test(Arc::clone(&ws)).await;

    // The loader reads the tree, says so, and waits for the test before it returns.
    let (started_tx, mut started) = tokio::sync::mpsc::unbounded_channel();
    let (gate, gate_rx) = std::sync::mpsc::channel::<()>();
    let gate_rx = std::sync::Mutex::new(gate_rx);
    let load: crate::workspace::types::RustLoader = Arc::new(move |root: &Path| {
        let engine = prod_code_engine_rust::RustEngine::load(root)?;
        let _ = started_tx.send(());
        let _ = gate_rx.lock().unwrap().recv();
        Ok(engine)
    });
    let admission = super::fixtures::roomy_admission();
    let validating = {
        let ws = Arc::clone(&ws);
        tokio::spawn(async move { ws.validation_engine(&admission, load).await })
    };
    started.recv().await.expect("the load read the tree");

    let lib = root.join("src/lib.rs");
    std::fs::write(&lib, "pub fn one() -> u8 { 1 }\npub fn two() -> u8 { 2 }\n").unwrap();
    let reached = manager
        .rust_engines_for_changes(&root, std::slice::from_ref(&lib))
        .await;
    assert_eq!(reached.len(), 1, "only the main engine is published yet");
    gate.send(()).unwrap();

    let engine = validating
        .await
        .unwrap()
        .expect("the validation engine loads");
    let names: Vec<String> = engine
        .lock()
        .await
        .snapshot()
        .document_symbols(&lib)
        .unwrap()
        .into_iter()
        .map(|symbol| symbol.name)
        .collect();
    assert!(names.iter().any(|n| n == "two"), "{names:?}");
}
