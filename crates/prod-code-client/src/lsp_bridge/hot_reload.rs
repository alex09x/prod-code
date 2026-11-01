/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::reconnect::replay_lsp_state;
use super::reconnect_deferred::prepare_deferred;
use super::reconnect_pending::replay_pending_requests;
use super::state::LspStateTracker;
use super::transport::{EditorFrameReceiver, LspTrace, PendingRequests, trace_reconnect};
use anyhow::{Context, Result};
use futures_util::SinkExt;
use futures_util::stream::{SplitSink, SplitStream};
use prod_code_protocol::{ProdCodeCodec, WireMessage};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU8;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::Stdout;
use tokio::sync::Mutex;
use tokio_util::codec::Framed;

pub const RESUMED_STATE_ENV: &str = "PROD_CODE_LSP_RESUME_FILE";
pub const WATCH_PATH_ENV: &str = "PROD_CODE_LSP_WATCH_PATH";
pub const WATCH_INTERVAL_ENV: &str = "PROD_CODE_LSP_WATCH_INTERVAL_MS";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutableFingerprint {
    #[cfg(unix)]
    pub dev: u64,
    #[cfg(unix)]
    pub ino: u64,
    pub len: u64,
    #[cfg(unix)]
    pub mtime_sec: i64,
    #[cfg(unix)]
    pub mtime_nsec: i64,
    #[cfg(not(unix))]
    pub modified: Option<SystemTime>,
}

impl ExecutableFingerprint {
    pub fn capture(path: &Path) -> Option<Self> {
        let meta = std::fs::metadata(path).ok()?;
        if !meta.is_file() || meta.len() == 0 {
            return None;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if (meta.mode() & 0o111) == 0 {
                return None;
            }
            Some(Self {
                dev: meta.dev(),
                ino: meta.ino(),
                len: meta.len(),
                mtime_sec: meta.mtime(),
                mtime_nsec: meta.mtime_nsec(),
            })
        }
        #[cfg(not(unix))]
        {
            Some(Self {
                len: meta.len(),
                modified: meta.modified().ok(),
            })
        }
    }
}

pub fn watched_executable_path() -> Option<PathBuf> {
    if let Some(override_path) = std::env::var_os(WATCH_PATH_ENV) {
        let path = PathBuf::from(override_path);
        if !path.as_os_str().is_empty() {
            return Some(path);
        }
    }
    let exe = std::env::current_exe().ok()?;
    let rendered = exe.to_string_lossy();
    if let Some(stripped) = rendered.strip_suffix(" (deleted)") {
        return Some(PathBuf::from(stripped));
    }
    Some(exe)
}

fn watch_interval() -> Duration {
    std::env::var(WATCH_INTERVAL_ENV)
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .map(|ms| Duration::from_millis(ms.max(20)))
        .unwrap_or_else(|| Duration::from_millis(2000))
}

pub fn spawn_executable_watcher() -> (
    tokio::task::JoinHandle<()>,
    tokio::sync::oneshot::Receiver<PathBuf>,
) {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let watched = watched_executable_path();
    let initial = watched.as_deref().and_then(ExecutableFingerprint::capture);
    let interval = watch_interval();
    let handle = tokio::spawn(async move {
        let (Some(path), Some(baseline)) = (watched, initial) else {
            return;
        };
        loop {
            tokio::time::sleep(interval).await;
            let Some(current) = ExecutableFingerprint::capture(&path) else {
                continue;
            };
            if current != baseline {
                tokio::time::sleep(Duration::from_millis(50)).await;
                let Some(settled) = ExecutableFingerprint::capture(&path) else {
                    continue;
                };
                if settled == current {
                    let _ = tx.send(path);
                    return;
                }
            }
        }
    });
    (handle, rx)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspResumeState {
    pub remote: SocketAddr,
    pub position_encoding: u8,
    pub tracker: LspStateTracker,
    pub deferred_editor_frames: Vec<String>,
    pub pending_requests: Vec<(serde_json::Value, String, String)>,
}

pub fn write_resume_state(state: &LspResumeState) -> Result<PathBuf> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let file_name = format!("prod-code-lsp-resume-{}-{nanos}.json", std::process::id());
    let path = std::env::temp_dir().join(file_name);
    let payload = serde_json::to_vec(state).context("failed to serialize LSP resume state")?;
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&path)
            .with_context(|| format!("failed to create resume state file {}", path.display()))?;
        file.write_all(&payload)
            .with_context(|| format!("failed to write resume state file {}", path.display()))?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&path, &payload)
            .with_context(|| format!("failed to write resume state file {}", path.display()))?;
    }
    Ok(path)
}

pub fn load_resume_state() -> Option<LspResumeState> {
    let path = PathBuf::from(std::env::var_os(RESUMED_STATE_ENV)?);
    // Safety: called at startup of `run_lsp_bridge` before worker tasks inspect this variable.
    unsafe {
        std::env::remove_var(RESUMED_STATE_ENV);
    }
    let bytes = std::fs::read(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    serde_json::from_slice(&bytes).ok()
}

pub async fn restore_resumed_session(
    socket_tx: &mut SplitSink<Framed<prod_code_protocol::AnyStream, ProdCodeCodec>, WireMessage>,
    socket_rx: &mut SplitStream<Framed<prod_code_protocol::AnyStream, ProdCodeCodec>>,
    tracker: &LspStateTracker,
    files: &prod_code_client::editor_files::RemoteFiles,
    position_encoding: &AtomicU8,
    editor_frames: &EditorFrameReceiver,
    deferred_editor_frames: &mut VecDeque<String>,
    pending_requests: &PendingRequests,
    editor_out: &Mutex<Stdout>,
    trace: &LspTrace,
) -> Result<bool> {
    let pending_initialize_id = {
        let pending = pending_requests.lock().await;
        pending
            .iter()
            .find(|(_, method, _)| method == "initialize")
            .map(|(id, _, _)| id.clone())
    };
    let replay_deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let replayed_initialize = tokio::time::timeout_at(
        replay_deadline,
        replay_lsp_state(
            socket_tx,
            socket_rx,
            tracker,
            pending_initialize_id.is_none(),
            files,
            position_encoding,
            editor_frames,
            deferred_editor_frames,
            pending_requests,
            editor_out,
            trace,
        ),
    )
    .await
    .context("timed out restoring LSP state after client hot-reload")??;

    if let Some(initialize_id) = pending_initialize_id
        && let Some(response) = replayed_initialize
    {
        let response = files.to_editor(response).await;
        let mut stdout = editor_out.lock().await;
        prod_code_client::editor_files::write_frame(&mut *stdout, &response)
            .await
            .context("forwarding replayed initialize response after hot-reload")?;
        drop(stdout);
        pending_requests
            .lock()
            .await
            .retain(|(id, method, _)| id != &initialize_id || method != "initialize");
    }

    tokio::time::timeout_at(
        replay_deadline,
        replay_pending_requests(
            socket_tx,
            files,
            pending_requests,
            editor_out,
            deferred_editor_frames,
        ),
    )
    .await
    .context("timed out answering in-flight requests after client hot-reload")??;

    tokio::time::timeout_at(
        replay_deadline,
        prepare_deferred(deferred_editor_frames, editor_out),
    )
    .await
    .context("timed out preparing queued editor frames after client hot-reload")??;

    trace_reconnect(trace, "hot_reload_restored", 1);
    Ok(!tracker.initialized_sent)
}

#[cfg(not(unix))]
pub async fn hot_reload_on_executable_update(
    _updated_exe: &Path,
    _remote: SocketAddr,
    _position_encoding: u8,
    _tracker: &LspStateTracker,
    _editor_frame_reader: &tokio::task::JoinHandle<()>,
    _editor_frames: &EditorFrameReceiver,
    _deferred_editor_frames: &mut VecDeque<String>,
    _pending_requests: &PendingRequests,
    _keeper: &tokio::task::JoinHandle<()>,
    _stdout_task: &tokio::task::JoinHandle<()>,
    _socket_tx: &mut SplitSink<Framed<prod_code_protocol::AnyStream, ProdCodeCodec>, WireMessage>,
    _trace: &LspTrace,
) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
pub async fn hot_reload_on_executable_update(
    updated_exe: &Path,
    remote: SocketAddr,
    position_encoding: u8,
    tracker: &LspStateTracker,
    editor_frame_reader: &tokio::task::JoinHandle<()>,
    editor_frames: &EditorFrameReceiver,
    deferred_editor_frames: &mut VecDeque<String>,
    pending_requests: &PendingRequests,
    keeper: &tokio::task::JoinHandle<()>,
    stdout_task: &tokio::task::JoinHandle<()>,
    socket_tx: &mut SplitSink<Framed<prod_code_protocol::AnyStream, ProdCodeCodec>, WireMessage>,
    trace: &LspTrace,
) -> Result<()> {
    ExecutableFingerprint::capture(updated_exe).with_context(|| {
        format!(
            "updated executable {} is missing or not executable",
            updated_exe.display()
        )
    })?;
    tokio::task::yield_now().await;
    editor_frame_reader.abort();
    {
        let mut rx = editor_frames.lock().await;
        while let Ok(Ok(Some(frame))) = rx.try_recv() {
            deferred_editor_frames.push_back(frame);
        }
    }
    let state = LspResumeState {
        remote,
        position_encoding,
        tracker: tracker.clone(),
        deferred_editor_frames: deferred_editor_frames.iter().cloned().collect(),
        pending_requests: pending_requests.lock().await.clone(),
    };
    let resume_path = write_resume_state(&state)?;
    let _ = socket_tx
        .send(WireMessage::Disconnect {
            reason: "client executable updated; hot-reloading bridge".to_string(),
        })
        .await;
    let _ = socket_tx.close().await;
    keeper.abort();
    stdout_task.abort();
    eprintln!(
        "prod-code lsp: client executable updated on disk; hot-reloading {}...",
        updated_exe.display()
    );
    trace_reconnect(trace, "hot_reload_exec", 1);
    // Safety: all worker tasks have been aborted before replacing the process image.
    unsafe {
        std::env::set_var(RESUMED_STATE_ENV, &resume_path);
    }
    let err = std::os::unix::process::CommandExt::exec(
        std::process::Command::new(updated_exe).args(std::env::args_os().skip(1)),
    );
    unsafe {
        std::env::remove_var(RESUMED_STATE_ENV);
    }
    let _ = std::fs::remove_file(&resume_path);
    Err(err).with_context(|| format!("failed to exec updated binary {}", updated_exe.display()))
}

#[cfg(test)]
#[path = "hot_reload_tests.rs"]
mod tests;
