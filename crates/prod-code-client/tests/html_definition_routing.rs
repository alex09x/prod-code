/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_testkit::{ScriptedGateway, Workspace};
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_1846_real_cli_defers_missing_html_capability_to_local_fallback() {
    let ws = Workspace::new(&[(
        "sample.html",
        "<script>function sortTable(index) { return index + 1; }</script>\n",
    )]);
    let node = ScriptedGateway::start(|_, _| serde_json::Value::Null).await;
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_prod-code"));
    command
        .args([
            "--remote",
            &node.addr().to_string(),
            "def",
            "sample.html",
            "--symbol",
            "sortTable",
        ])
        .current_dir(ws.root())
        .env_remove("PROD_CODE_PIN_NODE")
        .kill_on_drop(true);
    let out = tokio::time::timeout(Duration::from_secs(10), command.output())
        .await
        .unwrap()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.status.success(),
        "real CLI still refused before fallback: {text}"
    );
    assert!(
        text.contains("syntax-only") && text.contains("return index + 1"),
        "{text}"
    );
    assert_eq!(
        node.calls(),
        0,
        "source fallback must not pretend the Rust-only node served HTML"
    );
}
