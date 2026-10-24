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
use std::fs;

#[cfg(unix)]
#[test]
fn blocking_stdio_clears_only_the_nonblocking_flag() {
    crate::test_cache_support::isolated!();
    let descriptors = [libc::STDIN_FILENO, libc::STDOUT_FILENO, libc::STDERR_FILENO];
    let flags: Vec<_> = descriptors
        .iter()
        .map(|fd| unsafe { libc::fcntl(*fd, libc::F_GETFL) })
        .collect();
    for (fd, flags) in descriptors.iter().zip(&flags) {
        assert!(*flags >= 0);
        assert_eq!(
            unsafe { libc::fcntl(*fd, libc::F_SETFL, *flags | libc::O_NONBLOCK) },
            0
        );
    }
    ensure_blocking_stdio();
    for (fd, flags) in descriptors.iter().zip(&flags) {
        assert_eq!(
            unsafe { libc::fcntl(*fd, libc::F_GETFL) },
            *flags & !libc::O_NONBLOCK
        );
    }
    ensure_blocking_stdio();
    for (fd, flags) in descriptors.iter().zip(&flags) {
        assert_eq!(
            unsafe { libc::fcntl(*fd, libc::F_GETFL) },
            *flags & !libc::O_NONBLOCK
        );
    }
}

#[test]
fn session_and_running_entry_guards_balance_on_drop() {
    let count = AtomicUsize::new(4);
    let a = ActiveSession::start(&count);
    let b = ActiveSession::start(&count);
    assert_eq!(count.load(Ordering::Relaxed), 6);
    drop(a);
    assert_eq!(count.load(Ordering::Relaxed), 5);
    drop(b);
    assert_eq!(count.load(Ordering::Relaxed), 4);
    let entry = RunningEntry::start(
        Path::new("parent/coverage-process"),
        &["echo".into(), "hello world".into()],
    );
    {
        let entries = RUNNING_COMMANDS.lock().unwrap();
        let (workspace, command, _) = entries.get(&entry.0).unwrap();
        assert_eq!(workspace, "coverage-process");
        assert_eq!(command, "echo hello world");
    }
    assert!(
        running_commands()
            .iter()
            .any(|c| c.workspace == "coverage-process" && c.command == "echo hello world")
    );
    let id = entry.0;
    drop(entry);
    assert!(!RUNNING_COMMANDS.lock().unwrap().contains_key(&id));
}

#[cfg(unix)]
#[test]
fn sockets_reject_active_and_non_socket_paths_and_clean_only_the_bound_inode() {
    use std::os::unix::net::UnixListener;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("listener.sock");
    remove_stale_unix_socket(&path).unwrap();
    fs::write(&path, "keep").unwrap();
    assert_eq!(
        remove_stale_unix_socket(&path).unwrap_err().to_string(),
        format!("refusing to replace non-socket path {}", path.display())
    );
    assert_eq!(
        SocketCleaner::new(&path).err().unwrap().to_string(),
        "bound Unix socket path is not a socket"
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), "keep");
    fs::remove_file(&path).unwrap();
    let listener = UnixListener::bind(&path).unwrap();
    assert_eq!(
        remove_stale_unix_socket(&path).unwrap_err().to_string(),
        format!(
            "Unix socket {} is already accepting connections",
            path.display()
        )
    );
    drop(listener);
    remove_stale_unix_socket(&path).unwrap();
    assert!(!path.exists());
    let listener = UnixListener::bind(&path).unwrap();
    let cleaner = SocketCleaner::new(&path).unwrap();
    drop(cleaner);
    assert!(!path.exists());
    drop(listener);
    let listener = UnixListener::bind(&path).unwrap();
    let cleaner = SocketCleaner::new(&path).unwrap();
    fs::remove_file(&path).unwrap();
    fs::write(&path, "replacement").unwrap();
    drop(cleaner);
    assert_eq!(fs::read_to_string(&path).unwrap(), "replacement");
    drop(listener);
}
