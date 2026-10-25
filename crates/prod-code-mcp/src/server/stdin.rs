/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::reload::Reload;
use std::path::PathBuf;
use tokio::io::{AsyncBufRead, AsyncRead, BufReader};

/// Keep the outstanding-read ownership bit even when select drops this future.
pub(super) async fn fill<R: AsyncRead + Unpin>(
    reader: &mut BufReader<R>,
    pending: &mut Vec<u8>,
    read_pending: &mut bool,
) -> std::io::Result<usize> {
    std::future::poll_fn(|cx| {
        let polled = std::pin::Pin::new(&mut *reader).poll_fill_buf(cx);
        *read_pending = polled.is_pending();
        // Copy completed bytes before returning, without letting the reader borrow escape.
        polled.map(|result| {
            result.map(|buf| {
                pending.extend_from_slice(buf);
                buf.len()
            })
        })
    })
    .await
}

#[cfg(unix)]
pub(super) fn open(
    exe: Option<PathBuf>,
) -> std::io::Result<(impl tokio::io::AsyncRead + Unpin, Reload)> {
    use std::os::fd::AsFd;
    // The duplicate is close-on-exec; fd 0 itself remains available to the replacement.
    let file = std::fs::File::from(std::io::stdin().as_fd().try_clone_to_owned()?);
    super::stdin_unix::open(file, exe)
}

#[cfg(not(unix))]
pub(super) fn open(
    exe: Option<PathBuf>,
) -> std::io::Result<(impl tokio::io::AsyncRead + Unpin, Reload)> {
    if exe.is_some() {
        tracing::warn!("stdin hot reload is unavailable: process replacement requires Unix exec");
    }
    // hot_reload::reexec is Unsupported here. Continue normal stdin/EOF service without
    // issuing reload announcements for replacements this platform cannot perform.
    Ok((tokio::io::stdin(), Reload::new(None)))
}
