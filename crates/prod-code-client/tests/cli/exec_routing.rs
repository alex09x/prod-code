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
async fn explicit_exec_runs_on_a_node_without_the_workspace_language() {
    let ws = make_workspace();
    let gateway = MockGateway::start(|_, _| serde_json::Value::Null).await;
    *gateway.detected_engines.lock().unwrap() = Some(vec!["swift".to_string()]);
    *gateway.reported_platform.lock().unwrap() = Some("macos aarch64".to_string());

    let output = run_cli(
        &ws,
        gateway.addr,
        &["exec", "--", "cargo", "build", "--release"],
    )
    .await;

    assert!(
        output.status.success(),
        "explicit remote exec should not require Rust LSP:\n{}",
        stderr_of(&output)
    );
    assert!(stdout_of(&output).contains("test result: ok"));
}
