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

#[test]
fn issue_1802_immediate_swift_transport_failure_returns_an_explicit_local_outline() {
    let source = "public struct Demo {\n public func render() {}\n}\n";
    for error in [
        anyhow::anyhow!("Host is down (os error 64)"),
        anyhow::Error::new(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "refused",
        )),
    ] {
        let (symbols, note) = swift_result(Some(source), Err(error)).unwrap();
        assert_eq!(symbols, local_swift_outline_symbols(source));
        assert!(symbols.to_string().contains("Demo"));
        assert!(
            note.unwrap()
                .contains("lexical outline, not analyzer output")
        );
        assert!(note.unwrap().contains("prod-code cluster"));
    }
}

#[test]
fn issue_1802_swift_security_errors_and_missing_local_source_do_not_fake_success() {
    let error = swift_result(
        Some("struct Demo {}"),
        Err(anyhow::anyhow!("TLS certificate rejected")),
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("TLS certificate rejected"));
    assert!(format!("{error:#}").contains("prod-code cluster"));
    let nested = anyhow::Error::new(std::io::Error::new(
        std::io::ErrorKind::ConnectionReset,
        "reset",
    ))
    .context("TLS authentication failed");
    assert!(swift_result(Some("struct Demo {}"), Err(nested)).is_err());
    assert!(swift_result(None, Err(anyhow::anyhow!("Host is down (os error 64)"))).is_err());
    let native = serde_json::json!([{"name":"NativeOnly"}]);
    let (symbols, note) = swift_result(Some("struct Demo {}"), Ok(native.clone())).unwrap();
    assert_eq!(symbols, native);
    assert!(note.is_none());
}

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
