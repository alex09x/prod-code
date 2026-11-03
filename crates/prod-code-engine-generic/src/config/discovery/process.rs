/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A probe child that owns its process group until it is reaped (#1541, #1794).

use std::io::Read;
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Maximum number of bytes stored from stdout probe output.
pub const MAX_PROBE_STDOUT_BYTES: usize = 4096;

/// How long cleanup waits for the stdout reader after the group was killed.
const READER_GRACE: Duration = Duration::from_millis(100);

/// A child spawned as the leader of its own process group, with stdout collected.
///
/// Only [`OwnedProcessGroup::spawn`] makes one, so the group id is always the leader's
/// own PID. The leader is not reaped before its stdout reaches EOF, and once it is
/// reaped the group is never signalled again: a raw signal therefore only ever reaches
/// a PID that is still held by this child (alive or a zombie), never a reused one.
///
/// Limitation: a descendant that leaves the group (`setsid`, `setpgid`) escapes the
/// group kill. If it keeps stdout open, the reader thread is left to finish when that
/// descendant closes the pipe; cleanup waits for it only [`READER_GRACE`]. Memory used
/// for output stays bounded by [`MAX_PROBE_STDOUT_BYTES`]; excess output is discarded.
pub struct OwnedProcessGroup {
    child: Child,
    reaped: bool,
    stdout: Receiver<Vec<u8>>,
    reader: Option<JoinHandle<()>>,
}

impl OwnedProcessGroup {
    /// Spawns `cmd` in a new process group (Unix) with stdout piped to a bounded reader.
    pub fn spawn(cmd: &mut Command) -> std::io::Result<Self> {
        cmd.stdout(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let mut child = cmd.spawn()?;
        let pipe = child.stdout.take();
        let (tx, rx) = std::sync::mpsc::channel();
        let reader = std::thread::Builder::new()
            .name("npm-probe-stdout".to_string())
            .spawn(move || {
                let _ = tx.send(pipe.map(read_bounded).unwrap_or_default());
            });
        let mut group = Self {
            child,
            reaped: false,
            stdout: rx,
            reader: None,
        };
        match reader {
            Ok(handle) => {
                group.reader = Some(handle);
                Ok(group)
            }
            Err(e) => {
                group.terminate();
                Err(e)
            }
        }
    }

    /// The leader's PID, which is also the group id on Unix.
    pub fn id(&self) -> u32 {
        self.child.id()
    }

    /// Waits for stdout EOF and then the leader's exit, both before `deadline`.
    ///
    /// The state is polled at least once, also when `deadline` has already passed, and
    /// once more after every sleep before the deadline is declared expired. On expiry
    /// the still unreaped group is killed, the leader reaped, and `None` returned.
    pub fn wait_with_deadline(mut self, deadline: Instant) -> Option<Output> {
        let mut stdout = None;
        loop {
            if stdout.is_none() {
                stdout = match self.stdout.try_recv() {
                    Ok(bytes) => Some(bytes),
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => Some(Vec::new()),
                };
            }
            // Reap only after EOF: until then the leader keeps its PID, and with it
            // the group id, reserved for the group kill below.
            if let Some(bytes) = stdout.as_ref() {
                match self.child.try_wait() {
                    Ok(Some(status)) => {
                        self.reaped = true;
                        self.join_reader(Duration::ZERO);
                        return Some(Output {
                            status,
                            stdout: bytes.clone(),
                            stderr: Vec::new(),
                        });
                    }
                    Ok(None) => {}
                    Err(_) => {
                        // The leader's state is unknown; its PID may not be ours.
                        self.reaped = true;
                        return None;
                    }
                }
            }
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(10).min(deadline - now));
        }
        self.terminate();
        None
    }

    /// Kills the still unreaped group, reaps the leader and gives the reader a grace.
    fn terminate(&mut self) {
        if self.reaped {
            return;
        }
        #[cfg(unix)]
        if let Ok(pgid) = i32::try_from(self.child.id()) {
            #[cfg(test)]
            audit::record(pgid);
            unsafe extern "C" {
                fn kill(pid: i32, sig: i32) -> i32;
            }
            // SAFETY: plain syscall; the group id is the unreaped leader's own PID.
            unsafe {
                let _ = kill(-pgid, 9);
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.reaped = true;
        self.join_reader(READER_GRACE);
    }

    fn join_reader(&mut self, grace: Duration) {
        let finished = match self.stdout.recv_timeout(grace) {
            Ok(_) | Err(RecvTimeoutError::Disconnected) => true,
            Err(RecvTimeoutError::Timeout) => false,
        };
        // A reader that already delivered its output has finished, too.
        if (finished || grace.is_zero())
            && let Some(handle) = self.reader.take()
        {
            let _ = handle.join();
        }
    }
}

impl Drop for OwnedProcessGroup {
    fn drop(&mut self) {
        self.terminate();
    }
}

/// Waits for `group` for at most `timeout`; see [`OwnedProcessGroup::wait_with_deadline`].
pub fn wait_with_timeout(group: OwnedProcessGroup, timeout: Duration) -> Option<Output> {
    group.wait_with_deadline(Instant::now() + timeout)
}

/// Reads `stream` to EOF, keeping at most [`MAX_PROBE_STDOUT_BYTES`].
fn read_bounded(mut stream: impl Read) -> Vec<u8> {
    let mut collected = Vec::new();
    let mut buf = [0u8; 1024];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let room = MAX_PROBE_STDOUT_BYTES - collected.len();
                collected.extend_from_slice(&buf[..n.min(room)]);
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    collected
}

/// Test-only record of every group signal and whether its leader was still unreaped.
#[cfg(all(test, unix))]
pub(crate) mod audit {
    use std::cell::RefCell;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) struct GroupSignal {
        pub pgid: i32,
        /// `kill(pid, 0)` succeeded just before the signal: the leader still held its PID.
        pub leader_held: bool,
    }

    thread_local! {
        static SIGNALS: RefCell<Vec<GroupSignal>> = const { RefCell::new(Vec::new()) };
    }

    pub(super) fn record(pgid: i32) {
        unsafe extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }
        // SAFETY: signal 0 only checks that the PID exists.
        let leader_held = unsafe { kill(pgid, 0) == 0 };
        SIGNALS.with(|s| s.borrow_mut().push(GroupSignal { pgid, leader_held }));
    }

    /// Drains the signals recorded on the calling thread.
    pub(crate) fn take() -> Vec<GroupSignal> {
        SIGNALS.with(|s| std::mem::take(&mut *s.borrow_mut()))
    }
}
