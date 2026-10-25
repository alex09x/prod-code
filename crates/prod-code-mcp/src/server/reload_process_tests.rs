/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Real process stdin tests. A separate control socket establishes Pending and gates exec;
//! bytes are put in the pipe before the gate opens, without a sleep-based race.

use super::transport::serve_with_reload;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader, ReadBuf};

const MODE: &str = "PROD_CODE_TEST_STDIN_MODE";
const CONTROL: &str = "PROD_CODE_TEST_STDIN_CONTROL";
const TEST: &str = "server::reload_process_tests::server_reload_real_stdin";
const FIRST: &[u8] = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n";
const SECOND: &[u8] = b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n";

struct AnnouncePending<R> {
    inner: R,
    control: std::os::unix::net::UnixStream,
    requested: Arc<AtomicBool>,
    notify: Arc<tokio::sync::Notify>,
    announced: bool,
    wait_for_bytes: bool,
    saw_bytes: bool,
}

impl<R: AsyncRead + Unpin> AsyncRead for AnnouncePending<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let before = buf.filled().len();
        let result = Pin::new(&mut this.inner).poll_read(cx, buf);
        this.saw_bytes |= buf.filled().len() > before;
        if result.is_pending() && !this.announced && (!this.wait_for_bytes || this.saw_bytes) {
            this.announced = true;
            this.control.write_all(b"pending\n").unwrap();
            this.requested.store(true, Ordering::Release);
            this.notify.notify_one();
        }
        result
    }
}

fn flags() -> i32 {
    // SAFETY: fd 0 is inherited stdin in these isolated child processes.
    let flags = unsafe { libc::fcntl(0, libc::F_GETFL) };
    assert_ne!(flags, -1);
    flags
}

async fn child(mode: &str) {
    let original = flags();
    if std::env::var_os(crate::hot_reload::RESUMED_ENV).is_some() {
        assert_eq!(
            original & libc::O_NONBLOCK,
            0,
            "replacement inherited nonblocking stdin"
        );
        super::transport::run_stdio_mcp_server("127.0.0.1:1".parse().unwrap(), "/tmp".into())
            .await
            .unwrap();
        return;
    }
    let (input, mut reload) = super::stdin::open(None).unwrap();
    if mode.starts_with("socket") {
        assert_eq!(
            flags(),
            original,
            "socket stdin changed shared stdout flags"
        );
    }
    assert_eq!(reload.cancel_safe_input, mode != "file");
    let exe = if mode.ends_with("failed-exec") {
        std::path::PathBuf::from("/nonexistent-prod-code-test-executable")
    } else {
        std::env::current_exe().unwrap()
    };
    reload.exe = Some(exe);
    let mut control =
        std::os::unix::net::UnixStream::connect(std::env::var_os(CONTROL).unwrap()).unwrap();
    let mut reexec = reload.reexec;
    let callback_control = control.try_clone().unwrap();
    reload.reexec = Box::new(move |exe| {
        let mut control = &callback_control;
        control.write_all(b"exec\n").unwrap();
        let mut ack = [0];
        control.read_exact(&mut ack).unwrap();
        assert_eq!(ack, [b'!']);
        reexec(exe)
    });
    if mode == "file" {
        reload.requested.store(true, Ordering::Release);
        serve_with_reload(
            "127.0.0.1:1".parse().unwrap(),
            "/tmp".into(),
            false,
            BufReader::new(input),
            tokio::io::stdout(),
            reload,
        )
        .await
        .unwrap();
    } else {
        let input = AnnouncePending {
            inner: input,
            control: control.try_clone().unwrap(),
            requested: reload.requested.clone(),
            notify: reload.notify.clone(),
            announced: false,
            wait_for_bytes: mode == "partial",
            saw_bytes: false,
        };
        serve_with_reload(
            "127.0.0.1:1".parse().unwrap(),
            "/tmp".into(),
            false,
            BufReader::new(input),
            tokio::io::stdout(),
            reload,
        )
        .await
        .unwrap();
    }
    assert_eq!(
        flags(),
        original,
        "failed exec/EOF must restore stdin flags"
    );
    control.write_all(b"done\n").unwrap();
}

async fn exercise(mode: &str) {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("control");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", TEST, "--nocapture"])
        .env(MODE, mode)
        .env(CONTROL, &socket)
        .env_remove(crate::hot_reload::RESUMED_ENV)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut shared_socket = if mode.starts_with("socket") {
        let (parent, child) = std::os::unix::net::UnixStream::pair().unwrap();
        command.stdin(std::process::Stdio::from(std::os::fd::OwnedFd::from(
            child.try_clone().unwrap(),
        )));
        command.stdout(std::process::Stdio::from(std::os::fd::OwnedFd::from(child)));
        parent.set_nonblocking(true).unwrap();
        Some(tokio::net::UnixStream::from_std(parent).unwrap())
    } else {
        None
    };
    if mode == "file" {
        let path = dir.path().join("requests");
        std::fs::write(&path, [FIRST, SECOND].concat()).unwrap();
        command.stdin(std::fs::File::open(path).unwrap());
    } else {
        if shared_socket.is_none() {
            command.stdin(std::process::Stdio::piped());
        }
    }
    let mut process = command.spawn().unwrap();
    drop(command);
    let mut stdin = process.stdin.take();
    let interaction = async {
        if mode == "partial" {
            stdin
                .as_mut()
                .unwrap()
                .write_all(&FIRST[..20])
                .await
                .unwrap();
        }
        let (control, _) = listener.accept().await.unwrap();
        let mut control = BufReader::new(control);
        let mut line = String::new();
        if mode != "file" {
            control.read_line(&mut line).await.unwrap();
            assert_eq!(line, "pending\n");
            line.clear();
            if mode == "partial" {
                stdin
                    .as_mut()
                    .unwrap()
                    .write_all(&FIRST[20..])
                    .await
                    .unwrap();
            }
        }
        control.read_line(&mut line).await.unwrap();
        assert_eq!(line, "exec\n");
        // The old process has polled stdin Pending. Exec is now gated, and these bytes
        // must remain readable by the replacement (or the original after exec fails).
        if let Some(mut stdin) = stdin.take() {
            stdin
                .write_all(if mode == "partial" { SECOND } else { FIRST })
                .await
                .unwrap();
            stdin.flush().await.unwrap();
            drop(stdin);
        }
        if let Some(socket) = &mut shared_socket {
            socket.write_all(FIRST).await.unwrap();
            socket.shutdown().await.unwrap();
        }
        control.get_mut().write_all(b"!").await.unwrap();
        if mode.ends_with("failed-exec") {
            line.clear();
            control.read_line(&mut line).await.unwrap();
            assert_eq!(line, "done\n");
        }
    };
    if tokio::time::timeout(std::time::Duration::from_secs(10), interaction)
        .await
        .is_err()
    {
        process.kill().await.unwrap();
        let output = process.wait_with_output().await.unwrap();
        panic!("{mode}: control handshake timed out: {output:?}");
    }
    let mut socket_output = Vec::new();
    if let Some(socket) = &mut shared_socket {
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            tokio::io::AsyncReadExt::read_to_end(socket, &mut socket_output),
        )
        .await
        .unwrap()
        .unwrap();
    }
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        process.wait_with_output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(output.status.success(), "{mode}: {output:?}");
    let stdout = String::from_utf8(if shared_socket.is_some() {
        socket_output
    } else {
        output.stdout
    })
    .unwrap();
    let frames: Vec<Value> = stdout
        .lines()
        .filter(|line| line.starts_with('{'))
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let ids: Vec<_> = frames
        .iter()
        .filter_map(|frame| frame.get("id").cloned())
        .collect();
    for response in frames.iter().filter(|frame| frame.get("id").is_some()) {
        assert_eq!(response["result"], json!({}), "{mode}: {response:?}");
    }
    assert_eq!(
        ids,
        if mode == "file" || mode == "partial" {
            vec![json!(1), json!(2)]
        } else {
            vec![json!(1)]
        },
        "{mode}: {frames:?}"
    );
    assert_eq!(
        frames
            .iter()
            .filter(|frame| frame["method"] == "notifications/tools/list_changed")
            .count(),
        if mode.ends_with("failed-exec") { 1 } else { 2 },
        "{mode}: {frames:?}"
    );
    if mode == "partial" {
        assert_eq!(
            frames[0]["id"], 1,
            "consumed partial frame must finish before exec"
        );
    }
}

#[tokio::test]
async fn server_reload_real_stdin() {
    if let Ok(mode) = std::env::var(MODE) {
        child(&mode).await;
        return;
    }
    for mode in [
        "socket",
        "socket-failed-exec",
        "idle-pipe",
        "partial",
        "failed-exec",
        "file",
    ] {
        exercise(mode).await;
    }
}

#[tokio::test]
async fn server_reload_device_eof_and_regular_file_input() {
    let (mut input, reload) =
        super::stdin_unix::open(std::fs::File::open("/dev/null").unwrap(), None).unwrap();
    let mut bytes = Vec::new();
    tokio::io::AsyncReadExt::read_to_end(&mut input, &mut bytes)
        .await
        .unwrap();
    assert!(bytes.is_empty());
    drop(reload);
    let mut file = tempfile::tempfile().unwrap();
    file.write_all(FIRST).unwrap();
    std::io::Seek::rewind(&mut file).unwrap();
    let raw = file.as_raw_fd();
    let (mut input, reload) = super::stdin_unix::open(file, None).unwrap();
    assert!(!reload.cancel_safe_input);
    assert!(matches!(input, super::stdin_unix::Input::Blocking(_)));
    tokio::io::AsyncReadExt::read_to_end(&mut input, &mut bytes)
        .await
        .unwrap();
    assert_eq!(bytes, FIRST);
    // SAFETY: input still owns this regular-file descriptor.
    assert_eq!(
        unsafe { libc::fcntl(raw, libc::F_GETFL) } & libc::O_NONBLOCK,
        0
    );
}
