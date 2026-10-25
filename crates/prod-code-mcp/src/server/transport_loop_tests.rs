/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The loop's own clock and its process boundary: the cluster ticks while it serves, and the
//! stdio server runs on the real standard streams.

use super::transport_serve_tests::{next_response, nowhere, send};
use serde_json::json;
use tokio::io::{AsyncWriteExt, BufReader};

#[tokio::test]
async fn the_loop_keeps_serving_while_it_asks_the_cluster_for_a_better_node() {
    // A directory that is neither a repository nor a project: every tick is a no-op.
    let root = tempfile::tempdir().unwrap();
    let mut reload = super::reload::Reload::new(None);
    reload.rebalance_every = std::time::Duration::from_millis(20);
    let (client, server) = tokio::io::duplex(64 * 1024);
    let (read, write) = tokio::io::split(server);
    let handle = tokio::spawn(super::transport::serve_with_reload(
        nowhere(),
        root.path().to_path_buf(),
        false,
        BufReader::new(read),
        write,
        reload,
    ));
    let mut client = BufReader::new(client);
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    send(&mut client, r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#).await;
    assert_eq!(
        next_response(&mut client).await,
        json!({"jsonrpc":"2.0","id":1,"result":{}})
    );
    drop(client);
    assert!(handle.await.unwrap().is_ok());
}

/// `run_stdio_mcp_server` reads the process's own stdin and writes its stdout, so it runs in a
/// child of this test binary: a resumed session announces its tools, answers, and stops when
/// stdin ends.
#[tokio::test]
async fn the_stdio_server_announces_a_resumed_session_answers_and_stops_at_the_end_of_stdin() {
    const CHILD: &str = "PROD_CODE_TEST_STDIO_SERVER_CHILD";
    const TEST: &str = "server::transport_loop_tests::the_stdio_server_announces_a_resumed_session_answers_and_stops_at_the_end_of_stdin";
    if std::env::var_os(CHILD).is_some() {
        super::transport::run_stdio_mcp_server(nowhere(), "/tmp".into())
            .await
            .unwrap();
        return;
    }
    let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", TEST, "--nocapture"])
        .env(CHILD, "1")
        .env(crate::hot_reload::RESUMED_ENV, "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n")
        .await
        .unwrap();
    drop(stdin);
    let output = tokio::time::timeout(std::time::Duration::from_secs(30), child.wait_with_output())
        .await
        .expect("the server stops at the end of stdin")
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    let announced = crate::hot_reload::tools_list_changed();
    let (before, after) = stdout.split_once(announced.trim_end()).expect(&stdout);
    assert!(
        !before.contains("\"id\":1"),
        "the announcement comes first: {stdout}"
    );
    assert!(
        after.contains("{\"id\":1,\"jsonrpc\":\"2.0\",\"result\":{}}\n"),
        "{stdout}"
    );
}
