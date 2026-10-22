/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;
use std::time::Duration;

#[tokio::test]
async fn outline_timeout_regression_bounds_cold_workspace_sync() {
    let ws = prod_code_testkit::Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"cold\"\nversion = \"0.1.0\"\n",
        ),
        ("tests/integration.rs", "#[test]\nfn cold_test() {}\n"),
    ]);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let remote = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (_socket, _) = listener.accept().await.unwrap();
        std::future::pending::<()>().await;
    });
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        outline_query(
            remote,
            &ws.root(),
            &ws.path("tests/integration.rs"),
            serde_json::json!({}),
            Duration::from_millis(40),
        ),
    )
    .await;
    server.abort();
    let _ = server.await;
    let error = result
        .expect("outline must honor its own deadline")
        .unwrap_err()
        .to_string();
    assert!(error.contains("outline timed out"), "{error}");
    assert!(
        error.contains("sync, engine load or another query"),
        "{error}"
    );
    assert!(error.contains("prod-code cluster"), "{error}");
    assert!(error.contains("retry"), "{error}");
}

#[tokio::test]
async fn outline_missing_path_regression_suggests_split_module() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src/manager")).unwrap();
    std::fs::write(dir.path().join("src/manager/mod.rs"), "pub fn load() {}\n").unwrap();
    let error = outline_file(
        "127.0.0.1:1".parse().unwrap(),
        dir.path(),
        &dir.path().join("src/manager.rs"),
        "src/manager.rs",
        &OutlineOptions::all(3, false, "pass include_locals: true"),
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(error.contains("file does not exist"), "{error}");
    assert!(
        error.contains("did you mean `src/manager/mod.rs`"),
        "{error}"
    );
    assert!(!error.contains("URI"), "{error}");
}
