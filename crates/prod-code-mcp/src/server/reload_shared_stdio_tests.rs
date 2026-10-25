/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Exercise production stdout while stdin aliases its open-file description.

use std::future::Future;
use std::io::Write;
use std::os::fd::OwnedFd;
use std::task::Poll;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, BufReader, ReadBuf};

const TEST: &str = "server::reload_shared_stdio_tests::server_reload_shared_stdout_backpressure";
const CONTROL: &str = "PROD_CODE_TEST_SHARED_CONTROL";

async fn child(path: std::ffi::OsString) {
    // SAFETY: the child inherits valid stdin/stdout descriptors from a socket pair.
    let original = unsafe { libc::fcntl(1, libc::F_GETFL) };
    let (mut input, reload) = super::stdin::open(None).unwrap();
    assert!(reload.cancel_safe_input);
    assert_eq!(unsafe { libc::fcntl(1, libc::F_GETFL) }, original);
    std::future::poll_fn(|cx| {
        let mut storage = [0; 1];
        let mut buf = ReadBuf::new(&mut storage);
        assert!(
            std::pin::Pin::new(&mut input)
                .poll_read(cx, &mut buf)
                .is_pending()
        );
        Poll::Ready(())
    })
    .await;
    let mut control = std::os::unix::net::UnixStream::connect(path).unwrap();
    // Fill the real socket without changing its status flags. The peer waits for our
    // explicit blocked message before reading any stdout, so backpressure is certain.
    let filler = [b'x'; 4096];
    let mut count = 0;
    loop {
        // SAFETY: send reads only this live buffer; MSG_DONTWAIT affects this call alone.
        let n = unsafe { libc::send(1, filler.as_ptr().cast(), filler.len(), libc::MSG_DONTWAIT) };
        if n < 0 {
            assert_eq!(
                std::io::Error::last_os_error().kind(),
                std::io::ErrorKind::WouldBlock
            );
            break;
        }
        assert!(n > 0);
        count += n as usize;
    }
    assert!(count > 0);
    let response = serde_json::json!({"jsonrpc":"2.0", "id":7, "result":"y".repeat(1024 * 1024)})
        .to_string()
        + "\n";
    let mut calls = super::reload::ToolCalls::new();
    calls.spawn(async move { (Some("7".into()), None, Some(response)) });
    // Ensure the completed response is available before polling the drain.
    tokio::task::yield_now().await;
    let mut active = super::reload::ActiveCalls::new();
    let mut remote = "127.0.0.1:1".parse().unwrap();
    let mut output = tokio::io::stdout();
    let mut drain = Box::pin(super::reload::drain_for_reload(
        &mut calls,
        &mut active,
        &mut remote,
        &mut output,
        std::time::Duration::from_secs(5),
    ));
    std::future::poll_fn(|cx| {
        assert!(drain.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    writeln!(control, "blocked {count}").unwrap();
    drain.await.unwrap();
    assert_eq!(unsafe { libc::fcntl(1, libc::F_GETFL) }, original);
}

#[tokio::test]
async fn server_reload_shared_stdout_backpressure() {
    if let Some(path) = std::env::var_os(CONTROL) {
        child(path).await;
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("control");
    let listener = tokio::net::UnixListener::bind(&path).unwrap();
    let (parent, socket) = std::os::unix::net::UnixStream::pair().unwrap();
    parent.set_nonblocking(true).unwrap();
    let mut parent = tokio::net::UnixStream::from_std(parent).unwrap();
    let process = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", TEST, "--nocapture"])
        .env(CONTROL, path)
        .stdin(std::process::Stdio::from(OwnedFd::from(
            socket.try_clone().unwrap(),
        )))
        .stdout(std::process::Stdio::from(OwnedFd::from(socket)))
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let interaction = async {
        let (control, _) = listener.accept().await.unwrap();
        let mut line = String::new();
        BufReader::new(control).read_line(&mut line).await.unwrap();
        let count: usize = line
            .strip_prefix("blocked ")
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let mut bytes = Vec::new();
        parent.read_to_end(&mut bytes).await.unwrap();
        let stdout = String::from_utf8(bytes).unwrap();
        let start = stdout.find("{\"id\":7,").unwrap();
        assert!(stdout[..start].ends_with(&"x".repeat(count)));
        let response: serde_json::Value =
            serde_json::from_str(stdout[start..].lines().next().unwrap()).unwrap();
        assert_eq!(response["id"], 7);
        assert_eq!(
            response["result"].as_str().unwrap(),
            "y".repeat(1024 * 1024)
        );
        let output = process.wait_with_output().await.unwrap();
        assert!(output.status.success(), "{output:?}");
    };
    tokio::time::timeout(std::time::Duration::from_secs(15), interaction)
        .await
        .unwrap();
}
