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

#[tokio::test]
async fn cli_test_file_reports_the_exact_cargo_target() {
    let ws = make_workspace();
    ws.write("tests/parity_scroll.rs", "#[test] fn selected() {}\n");
    let gw = MockGateway::start(|_, _| serde_json::Value::Null).await;
    let output = run_cli(
        &ws,
        gw.addr,
        &["test", "--path", "tests/parity_scroll.rs", "--json"],
    )
    .await;
    assert!(output.status.success(), "{}", stderr_of(&output));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report["command"],
        serde_json::json!(["cargo", "test", "-p", "fixture", "--test", "parity_scroll"])
    );
}

#[cfg(unix)]
#[tokio::test]
async fn cli_refuses_a_symlinked_test_target() {
    let ws = make_workspace();
    ws.write("tests/real.rs", "#[test] fn selected() {}\n");
    std::os::unix::fs::symlink("real.rs", ws.path("tests/alias.rs")).unwrap();
    std::os::unix::fs::symlink("real.rs", ws.path("tests/alias")).unwrap();
    let gw = MockGateway::start(|_, _| serde_json::Value::Null).await;
    for alias in ["tests/alias.rs", "tests/alias"] {
        let output = run_cli(&ws, gw.addr, &["test", "--path", alias]).await;
        assert!(!output.status.success());
        assert!(
            stderr_of(&output).contains("symlink"),
            "{}",
            stderr_of(&output)
        );
    }
}
