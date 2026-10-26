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
use tokio::io::AsyncReadExt;

pub const EXEC_DEFAULT_TIMEOUT_SECS: u64 = 3600;
pub const MAX_REMOTE_EXEC_TIMEOUT_SECS: u64 = 86400 * 7;
pub const MAX_JSON_LINE_BUFFER_BYTES: usize = 1024 * 1024;

/// Whether `program` is an executable file in a directory of `PATH`.
pub fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

/// Blocks until the child `pid` has exited and leaves it unreaped (`WNOWAIT`): its zombie
/// keeps the PID, and with it the number of the group it leads, until its reaper collects it
/// (#1545). False when `pid` cannot be waited for.
pub fn wait_for_exit(pid: i32) -> bool {
    loop {
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let got = unsafe {
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOWAIT,
            )
        };
        if got == 0 {
            return true;
        }
        if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return false;
        }
    }
}

/// Kills the command's process group and its leader. Called only while the leader is not
/// reaped: alive or a zombie, it holds its PID, so neither that PID nor the group's number can
/// belong to another process yet (#1545). The group is signalled after the leader exited too,
/// because descendants that outlived it may hold its pipes (#1541).
pub fn kill_exec_group(pid: u32) {
    let _ = std::process::Command::new("kill")
        .args(["-9", "--", &format!("-{pid}")])
        .stderr(std::process::Stdio::null())
        .status();
    let _ = std::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status();
}

pub fn resolve_run_dir(workspace: &Path, subdir: Option<&str>) -> PathBuf {
    match subdir {
        Some(sub)
            if !sub.is_empty()
                && !sub.starts_with('/')
                && !sub.split('/').any(|c| c == "..")
                && workspace.join(sub).is_dir() =>
        {
            workspace.join(sub)
        }
        _ => workspace.to_path_buf(),
    }
}

pub fn spawn_pipe_readers(
    child: &mut std::process::Child,
    tx: rapidfire::mpsc::Sender<ExecChunk>,
) -> Vec<tokio::task::JoinHandle<()>> {
    let mut readers = Vec::new();
    if let Some(mut out) = child
        .stdout
        .take()
        .and_then(|o| tokio::process::ChildStdout::from_std(o).ok())
    {
        let tx = tx.clone();
        readers.push(tokio::spawn(async move {
            let mut buf = vec![0u8; 16 * 1024];
            while let Ok(n) = out.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                if tx
                    .send(ExecChunk {
                        stderr: false,
                        data: Some(buf[..n].to_vec()),
                    })
                    .await
                    .is_err()
                {
                    break;
                }
            }
        }));
    }
    if let Some(mut err) = child
        .stderr
        .take()
        .and_then(|e| tokio::process::ChildStderr::from_std(e).ok())
    {
        let tx = tx.clone();
        readers.push(tokio::spawn(async move {
            let mut buf = vec![0u8; 16 * 1024];
            while let Ok(n) = err.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                if tx
                    .send(ExecChunk {
                        stderr: true,
                        data: Some(buf[..n].to_vec()),
                    })
                    .await
                    .is_err()
                {
                    break;
                }
            }
        }));
    }
    readers
}

pub enum DiskCheckOutcome {
    Ok,
    Warn(String),
    Refuse(String),
}

pub fn check_node_disk_headroom(workspace: &Path, storage_root: &Path) -> DiskCheckOutcome {
    if let Some((free, total)) = workspace::free_and_total_bytes(workspace)
        .or_else(|| workspace::free_and_total_bytes(storage_root))
    {
        let free_gb = free as f64 / (1024.0 * 1024.0 * 1024.0);
        let used_pct = if total > 0 {
            (1.0 - (free as f64 / total as f64)) * 100.0
        } else {
            0.0
        };
        let hostname = get_hostname();
        if free_gb < 5.0 || used_pct > 95.0 {
            return DiskCheckOutcome::Refuse(format!(
                "refused: node {hostname} has only {free_gb:.1} GB free on {} ({used_pct:.0}% full)",
                workspace.display()
            ));
        }
        if free_gb < 10.0 {
            return DiskCheckOutcome::Warn(format!(
                "[prod-code exec] WARNING: {free_gb:.1} GB free on node {hostname} ({used_pct:.0}% used)\n"
            ));
        }
    }
    DiskCheckOutcome::Ok
}

#[cfg(test)]
mod tests;
