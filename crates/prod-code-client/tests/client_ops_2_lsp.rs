/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_client::editor_files::{read_frame, write_frame};
use prod_code_testkit::{ScriptedGateway, Workspace};
use serde_json::{Value, json};
use std::process::Stdio;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};

fn spawn(
    ws: &Workspace,
    cache: &std::path::Path,
    gateway: &ScriptedGateway,
    watchdog: &str,
) -> tokio::process::Child {
    tokio::process::Command::new(env!("CARGO_BIN_EXE_prod-code"))
        .arg("--remote")
        .arg(gateway.addr().to_string())
        .args(["lsp", "--watchdog-secs", watchdog])
        .env("HOME", cache)
        .env("XDG_CACHE_HOME", cache)
        .env_remove("PROD_CODE_CLUSTER")
        .env("PROD_CODE_RECONNECT", "false")
        .current_dir(ws.root())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap()
}

#[tokio::test]
async fn watchdog_failure_answers_pending_and_cancelled_requests() {
    for cancel in [false, true] {
        let ws = Workspace::new(&[("src/lib.rs", "pub fn sample() {}\n")]);
        let cache = tempfile::tempdir().unwrap();
        let gateway = ScriptedGateway::start(|_, _| json!({"prod-code/no-response":true})).await;
        let mut child = spawn(&ws, cache.path(), &gateway, "1");
        let mut stdin = child.stdin.take().unwrap();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let init = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}});
        write_frame(&mut stdin, &init.to_string()).await.unwrap();
        let reply: Value =
            serde_json::from_str(&read_frame(&mut stdout).await.unwrap().unwrap()).unwrap();
        assert_eq!(
            reply,
            json!({"jsonrpc":"2.0","id":1,"result":{"capabilities":{"hoverProvider":true}}})
        );
        write_frame(
            &mut stdin,
            &json!({"jsonrpc":"2.0","id":7,"method":"textDocument/hover","params":{}}).to_string(),
        )
        .await
        .unwrap();
        if cancel {
            write_frame(
                &mut stdin,
                &json!({"jsonrpc":"2.0","method":"$/cancelRequest","params":{"id":7}}).to_string(),
            )
            .await
            .unwrap();
        }
        let status = tokio::time::timeout(std::time::Duration::from_secs(5), child.wait())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(status.code(), Some(1));
        if cancel {
            let reply: Value =
                serde_json::from_str(&read_frame(&mut stdout).await.unwrap().unwrap()).unwrap();
            assert_eq!(
                reply,
                json!({"jsonrpc":"2.0","id":7,"error":{"code":-32800,"message":"request cancelled"}})
            );
        } else {
            let reply: Value =
                serde_json::from_str(&read_frame(&mut stdout).await.unwrap().unwrap()).unwrap();
            assert_eq!(
                reply,
                json!({"jsonrpc":"2.0","id":7,"error":{"code":-32097,"message":"prod-code lsp: gateway connection lost (did not respond to watchdog ping within 1s)"}})
            );
        }
        let mut stderr = String::new();
        child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut stderr)
            .await
            .unwrap();
        assert!(stderr.lines().any(|line| line
            == format!(
                "prod-code lsp: the gateway at {} did not respond to watchdog ping within 1s",
                gateway.addr()
            )));
    }
}

#[tokio::test]
async fn malformed_editor_frame_reports_the_exact_protocol_error() {
    let ws = Workspace::new(&[("src/lib.rs", "pub fn sample() {}\n")]);
    let cache = tempfile::tempdir().unwrap();
    let gateway = ScriptedGateway::start(|_, _| Value::Null).await;
    let mut child = spawn(&ws, cache.path(), &gateway, "0");
    let mut stdin = child.stdin.take().unwrap();
    stdin
        .write_all(b"Content-Length: invalid\r\n\r\n")
        .await
        .unwrap();
    drop(stdin);
    let output = tokio::time::timeout(std::time::Duration::from_secs(5), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .lines()
            .any(|line| line == "Error: reading the editor's message: Invalid LSP Content-Length")
    );
    assert_eq!(output.stdout, b"");
}
