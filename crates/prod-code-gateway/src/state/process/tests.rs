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
    // Not `remove_stale_unix_socket` here: a process forked by a parallel test can still hold
    // the closed listener's descriptor for a moment, and the probe then sees it accepting.
    fs::remove_file(&path).unwrap();
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

#[cfg(unix)]
#[test]
fn a_socket_nobody_listens_on_is_removed_and_other_probe_failures_are_refused() {
    use std::os::unix::net::{UnixDatagram, UnixListener};
    let temp = tempfile::tempdir().unwrap();

    // A missing path is nothing to remove.
    remove_stale_unix_socket(&temp.path().join("absent.sock")).unwrap();

    // A path below a file cannot be inspected; the error is the system's, not a refusal.
    let file = temp.path().join("plain");
    fs::write(&file, "keep").unwrap();
    let error = remove_stale_unix_socket(&file.join("below.sock")).unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<std::io::Error>()
            .unwrap()
            .raw_os_error(),
        Some(libc::ENOTDIR)
    );
    assert_eq!(
        SocketCleaner::new(&temp.path().join("absent.sock"))
            .err()
            .unwrap()
            .kind(),
        std::io::ErrorKind::NotFound
    );

    // A socket of another type cannot be told stale: connecting to it is refused for a reason
    // other than nobody listening, and the path stays.
    let path = temp.path().join("datagram.sock");
    let datagram = UnixDatagram::bind(&path).unwrap();
    let error = remove_stale_unix_socket(&path).unwrap_err();
    assert_eq!(
        error.to_string(),
        format!(
            "cannot verify whether Unix socket {} is stale",
            path.display()
        )
    );
    assert_eq!(error.chain().count(), 2);
    assert!(path.exists());
    drop(datagram);
    fs::remove_file(&path).unwrap();

    // A socket whose listener has gone is stale and is removed. A process forked by a parallel
    // test can hold the closed listener's descriptor for a moment, and the probe then sees it
    // accepting; it lets go as soon as it has exec'd.
    let path = temp.path().join("stale.sock");
    drop(UnixListener::bind(&path).unwrap());
    let mut attempts = 0;
    loop {
        match remove_stale_unix_socket(&path) {
            Ok(()) => break,
            Err(error)
                if error
                    .to_string()
                    .ends_with("is already accepting connections") =>
            {
                attempts += 1;
                assert!(attempts < 500, "{error}");
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(error) => panic!("{error}"),
        }
    }
    assert!(!path.exists());
}

#[test]
fn the_host_name_is_a_real_name_and_running_commands_are_reported_with_their_age() {
    let name = get_hostname();
    assert!(!name.is_empty());
    assert!(!name.contains('\0'));
    assert!(!name.contains('\n'));

    let entry = RunningEntry::start(Path::new("/"), &[]);
    {
        let entries = RUNNING_COMMANDS.lock().unwrap();
        let (workspace, command, _) = entries.get(&entry.0).unwrap();
        assert_eq!((workspace.as_str(), command.as_str()), ("", ""));
    }
    let other = RunningEntry::start(Path::new("w/coverage-ages"), &["a".into(), "b".into()]);
    assert_ne!(entry.0, other.0);
    let listed: Vec<_> = running_commands()
        .into_iter()
        .filter(|c| c.workspace == "coverage-ages")
        .collect();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].command, "a b");
    assert!(listed[0].running_seconds < 60);
}
