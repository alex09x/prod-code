/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::*;

pub static NEXT_REQ_ID: AtomicU64 = AtomicU64::new(1);
pub static ACTIVE_QUERIES: AtomicUsize = AtomicUsize::new(0);

/// Remote commands running now, by a per-process id: workspace directory name, command line and
/// start. Status answers list them, so that a node running a build or test is not taken for
/// idle and restarted under it (#273).
pub(crate) type RunningTable = std::collections::HashMap<u64, (String, String, Instant)>;
pub(crate) static RUNNING_COMMANDS: std::sync::LazyLock<std::sync::Mutex<RunningTable>> =
    std::sync::LazyLock::new(Default::default);

#[cfg(unix)]
pub(crate) fn remove_stale_unix_socket(path: &Path) -> Result<()> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};

    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    anyhow::ensure!(
        metadata.file_type().is_socket(),
        "refusing to replace non-socket path {}",
        path.display()
    );
    match std::os::unix::net::UnixStream::connect(path) {
        Ok(_) => anyhow::bail!(
            "Unix socket {} is already accepting connections",
            path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
            let current = match std::fs::symlink_metadata(path) {
                Ok(current) => current,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(error.into()),
            };
            anyhow::ensure!(
                current.file_type().is_socket()
                    && current.dev() == metadata.dev()
                    && current.ino() == metadata.ino(),
                "Unix socket path {} changed while checking whether it is stale",
                path.display()
            );
            std::fs::remove_file(path)
                .with_context(|| format!("cannot remove stale Unix socket {}", path.display()))?;
            Ok(())
        }
        Err(error) => Err(error).with_context(|| {
            format!(
                "cannot verify whether Unix socket {} is stale",
                path.display()
            )
        }),
    }
}

#[cfg(unix)]
pub struct SocketCleaner {
    path: PathBuf,
    device: u64,
    inode: u64,
}

#[cfg(unix)]
impl SocketCleaner {
    pub fn new(path: &Path) -> std::io::Result<Self> {
        use std::os::unix::fs::{FileTypeExt, MetadataExt};

        let metadata = std::fs::symlink_metadata(path)?;
        if !metadata.file_type().is_socket() {
            return Err(std::io::Error::other(
                "bound Unix socket path is not a socket",
            ));
        }
        Ok(Self {
            path: path.to_path_buf(),
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
}

#[cfg(unix)]
impl Drop for SocketCleaner {
    fn drop(&mut self) {
        use std::os::unix::fs::{FileTypeExt, MetadataExt};

        if std::fs::symlink_metadata(&self.path).is_ok_and(|metadata| {
            metadata.file_type().is_socket()
                && metadata.dev() == self.device
                && metadata.ino() == self.inode
        }) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}
pub(crate) static NEXT_COMMAND_ID: AtomicU64 = AtomicU64::new(0);

/// A command's entry in [`RUNNING_COMMANDS`], removed when the command's handler returns,
/// however it returns.
pub struct RunningEntry(pub u64);

impl RunningEntry {
    pub fn start(workspace: &std::path::Path, command: &[String]) -> Self {
        let id = NEXT_COMMAND_ID.fetch_add(1, Ordering::Relaxed);
        let name = workspace
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        RUNNING_COMMANDS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, (name, command.join(" "), Instant::now()));
        Self(id)
    }
}

impl Drop for RunningEntry {
    fn drop(&mut self) {
        RUNNING_COMMANDS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0);
    }
}

/// The commands running now, as a status answer reports them.
pub(crate) fn running_commands() -> Vec<prod_code_protocol::RunningCommand> {
    RUNNING_COMMANDS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .values()
        .map(
            |(workspace, command, started)| prod_code_protocol::RunningCommand {
                workspace: workspace.clone(),
                command: command.clone(),
                running_seconds: started.elapsed().as_secs(),
            },
        )
        .collect()
}
pub static TOTAL_QUERIES: AtomicU64 = AtomicU64::new(0);
pub static SLOW_QUERIES: AtomicU64 = AtomicU64::new(0);

/// Ensure standard I/O file descriptors are in blocking mode (#806, #807).
///
/// When the gateway daemon runs under systemd with journald or under piped supervision,
/// some supervisors or previous subprocesses may leave stdin, stdout, or stderr with O_NONBLOCK set.
/// If tracing or rust-analyzer writes to a non-blocking stderr while the socket buffer is full,
/// standard library `eprintln!` panics with EAGAIN ("os error 11: Resource temporarily unavailable").
#[cfg(unix)]
pub fn ensure_blocking_stdio() {
    for fd in [libc::STDIN_FILENO, libc::STDOUT_FILENO, libc::STDERR_FILENO] {
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            if flags >= 0 && (flags & libc::O_NONBLOCK) != 0 {
                libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK);
            }
        }
    }
}

#[cfg(not(unix))]
pub fn ensure_blocking_stdio() {}

#[cfg(unix)]
pub(crate) fn get_hostname() -> String {
    let mut buf = [0u8; 256];
    if unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) } == 0
        && let Some(pos) = buf.iter().position(|&b| b == 0)
    {
        return String::from_utf8_lossy(&buf[..pos]).to_string();
    }
    std::env::var("HOSTNAME").unwrap_or_else(|_| "node".to_string())
}

#[cfg(not(unix))]
pub(crate) fn get_hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "node".to_string())
}

pub struct ActiveSession<'a>(&'a AtomicUsize);

impl<'a> ActiveSession<'a> {
    pub fn start(counter: &'a AtomicUsize) -> Self {
        counter.fetch_add(1, Ordering::Relaxed);
        Self(counter)
    }
}

impl Drop for ActiveSession<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}
