/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{reload::Reload, transport::serve_with_reload};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, BufReader, ReadBuf};

/// Models a blocking worker that already owns bytes, but has not delivered its result.
/// Completion is controlled by the test, not scheduler timing or a sleep.
struct PrivateRead {
    polled: Arc<AtomicBool>,
    complete: Arc<AtomicBool>,
    bytes: &'static [u8],
}

impl AsyncRead for PrivateRead {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        self.polled.store(true, Ordering::SeqCst);
        if !self.complete.load(Ordering::SeqCst) {
            return Poll::Pending;
        }
        buf.put_slice(self.bytes);
        self.bytes = b"";
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn server_reload_waits_for_privately_owned_pending_read() {
    let polled = Arc::new(AtomicBool::new(false));
    let complete = Arc::new(AtomicBool::new(false));
    let attempts = Arc::new(AtomicUsize::new(0));
    let mut reload = Reload::new(None);
    reload.exe = Some("unused-test-executable".into());
    let requested = reload.requested.clone();
    let notify = reload.notify.clone();
    let counted = attempts.clone();
    reload.reexec = Box::new(move |_| {
        counted.fetch_add(1, Ordering::SeqCst);
        std::io::Error::other("simulated exec failure")
    });
    let root = tempfile::tempdir().unwrap();
    let mut output = Vec::new();
    let mut server = Box::pin(serve_with_reload(
        "127.0.0.1:1".parse().unwrap(),
        root.path().into(),
        false,
        BufReader::new(PrivateRead {
            polled: polled.clone(),
            complete: complete.clone(),
            bytes: b"{\"jsonrpc\":\"2.0\",\"id\":\"private\",\"method\":\"ping\"}\n",
        }),
        &mut output,
        reload,
    ));
    assert!(futures_util::poll!(&mut server).is_pending());
    assert!(polled.load(Ordering::SeqCst), "read must be outstanding");
    requested.store(true, Ordering::Release);
    notify.notify_one();
    assert!(futures_util::poll!(&mut server).is_pending());
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        0,
        "exec would discard the worker's privately owned request"
    );
    complete.store(true, Ordering::SeqCst);
    server.await.unwrap();
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    let frames: Vec<serde_json::Value> = output
        .split(|b| *b == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).unwrap())
        .collect();
    assert_eq!(frames.len(), 2, "{frames:?}");
    assert_eq!(frames[0]["id"], "private");
    assert_eq!(frames[0]["result"], serde_json::json!({}));
    assert_eq!(frames[1]["method"], "notifications/tools/list_changed");
}
