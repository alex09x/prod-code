/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Readiness input never starts a blocking worker. Pending leaves bytes in the OS.
//! Sockets use per-call nonblocking reads, preserving shared output status flags.
//! Regular files, aliased non-socket streams and reactor-rejected devices retain
//! asynchronous worker reads with conservative reload ownership.

use super::reload::Reload;
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, ready};
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncRead, ReadBuf};

pub(super) enum Input {
    Ready(AsyncFd<SharedDescriptor>),
    Blocking(tokio::fs::File),
}

pub(super) struct SharedDescriptor(Arc<Descriptor>);

struct Descriptor {
    file: File,
    original_flags: libc::c_int,
    socket: bool,
    unusable: AtomicBool,
}

impl AsRawFd for SharedDescriptor {
    fn as_raw_fd(&self) -> RawFd {
        self.0.file.as_raw_fd()
    }
}

impl Descriptor {
    fn set_flags(&self, flags: libc::c_int) -> io::Result<()> {
        if self.socket {
            return Ok(());
        }
        // SAFETY: the owned file keeps this descriptor valid; F_SETFL takes an integer.
        if unsafe { libc::fcntl(self.file.as_raw_fd(), libc::F_SETFL, flags) } == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn reexec(&self, exe: &std::path::Path) -> io::Error {
        // dup shares file-status flags with stdin. The new process must inherit the
        // original flags, and a failed exec must restore nonblocking reads here.
        // Sockets never change flags: their reads use MSG_DONTWAIT instead.
        if let Err(error) = self.set_flags(self.original_flags) {
            return error;
        }
        let error = crate::hot_reload::reexec(exe);
        if let Err(restore) = self.set_flags(self.original_flags | libc::O_NONBLOCK) {
            self.unusable.store(true, Ordering::Release);
            return restore;
        }
        error
    }
}

impl Drop for Descriptor {
    fn drop(&mut self) {
        let _ = self.set_flags(self.original_flags);
    }
}

pub(super) fn open(file: File, exe: Option<PathBuf>) -> io::Result<(Input, Reload)> {
    let mut reload = Reload::new(exe);
    let metadata = file.metadata()?;
    let socket = metadata.file_type().is_socket();
    if metadata.is_file() || (!socket && aliases_output(&metadata)) {
        // Keep slow file I/O off the runtime thread, without cancelling an owned read.
        // A shared terminal/FIFO must also retain its flags for blocking stdout/stderr.
        return Ok((Input::Blocking(tokio::fs::File::from_std(file)), reload));
    }
    // SAFETY: F_GETFL only queries flags on the owned, valid file descriptor.
    let original_flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
    if original_flags == -1 {
        return Err(io::Error::last_os_error());
    }
    let descriptor = Arc::new(Descriptor {
        file,
        original_flags,
        socket,
        unusable: AtomicBool::new(false),
    });
    descriptor.set_flags(original_flags | libc::O_NONBLOCK)?;
    match AsyncFd::new(SharedDescriptor(descriptor.clone())) {
        Ok(input) => {
            reload.cancel_safe_input = true;
            reload.reexec = Box::new(move |exe| descriptor.reexec(exe));
            Ok((Input::Ready(input), reload))
        }
        Err(error) => {
            descriptor.set_flags(original_flags)?;
            let file = descriptor.file.try_clone()?;
            // /dev/null and some device files cannot be registered with epoll/kqueue.
            // Their worker must finish before exec, even when outer buffers are empty.
            tracing::debug!(%error, "stdin requires a blocking reader; reload waits for its read");
            Ok((Input::Blocking(tokio::fs::File::from_std(file)), reload))
        }
    }
}

fn aliases_output(input: &std::fs::Metadata) -> bool {
    use std::os::fd::{AsFd, BorrowedFd};
    [std::io::stdout().as_fd(), std::io::stderr().as_fd()]
        .iter()
        .any(|fd: &BorrowedFd<'_>| {
            let other = fd
                .try_clone_to_owned()
                .and_then(|fd| File::from(fd).metadata());
            // Matching objects can have separate descriptions: conservative fallback is safe.
            match other {
                Ok(other) => input.dev() == other.dev() && input.ino() == other.ino(),
                Err(_) => true,
            }
        })
}

fn read(descriptor: &Descriptor, buf: &mut ReadBuf<'_>) -> io::Result<()> {
    loop {
        let result = if descriptor.socket {
            let bytes = buf.initialize_unfilled();
            // SAFETY: recv writes only within this live buffer. MSG_DONTWAIT applies to
            // this read alone, leaving any shared stdout/stderr description blocking.
            let count = unsafe {
                libc::recv(
                    descriptor.file.as_raw_fd(),
                    bytes.as_mut_ptr().cast(),
                    bytes.len(),
                    libc::MSG_DONTWAIT,
                )
            };
            if count < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(count as usize)
            }
        } else {
            (&descriptor.file).read(buf.initialize_unfilled())
        };
        match result {
            Ok(count) => {
                buf.advance(count);
                return Ok(());
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
}

impl AsyncRead for Input {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        match self.get_mut() {
            Self::Blocking(file) => Pin::new(file).poll_read(cx, buf),
            Self::Ready(input) => {
                if input.get_ref().0.unusable.load(Ordering::Acquire) {
                    return Poll::Ready(Err(io::Error::other("stdin flags could not be restored")));
                }
                loop {
                    let mut guard = ready!(input.poll_read_ready(cx))?;
                    match guard.try_io(|inner| read(&inner.get_ref().0, buf)) {
                        Ok(result) => return Poll::Ready(result),
                        Err(_) => continue,
                    }
                }
            }
        }
    }
}
