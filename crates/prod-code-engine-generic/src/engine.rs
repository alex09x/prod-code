/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result};
use prod_code_protocol::readiness::Readiness;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, RwLock, broadcast, oneshot};

use crate::config::GenericLspConfig;
use crate::diagnostics::{Published, Sent};
use crate::reader::{spawn_reader_loop, spawn_stderr_reader};
use crate::types::{
    DocumentLifecycle, HealthProbePending, HealthProbeTask, ProbeState, lock_unpoisoned,
};

/// Supervised generic language server process adapter.
pub struct GenericLspEngine {
    pub workspace_root: PathBuf,
    pub config: GenericLspConfig,
    pub(crate) stdin: Arc<Mutex<ChildStdin>>,
    pub(crate) next_req_id: Arc<AtomicU64>,
    pub(crate) pending_requests: Arc<StdMutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>>,
    pub capabilities: Arc<RwLock<Option<serde_json::Value>>>,
    pub(crate) broadcast_tx: broadcast::Sender<String>,
    pub(crate) last_activity: Arc<RwLock<Instant>>,
    /// Latest `textDocument/publishDiagnostics` per document URI, the context quick fixes
    /// (`textDocument/codeAction`) are computed from.
    pub(crate) diagnostics: Arc<RwLock<HashMap<String, Published>>>,
    /// The text last sent per document URI (`didOpen`, `didChange`), which a publication has
    /// to cover before it answers for the document (#293).
    pub(crate) sent: Arc<RwLock<HashMap<String, Sent>>>,
    /// Whether the server has published with a document version, as clangd does: then a
    /// publication without one (clangd's, for a document just closed) describes no text sent.
    pub(crate) versioned: Arc<AtomicBool>,
    /// Whether the server answered a diagnostic pull with "method not found" (clangd does).
    pub(crate) pull_unsupported: AtomicBool,
    /// Receives the `workspace/applyEdit` a server sends while a command runs.
    pub(crate) apply_edit_waiter: Arc<Mutex<Option<oneshot::Sender<serde_json::Value>>>>,
    pub(crate) is_alive: Arc<AtomicBool>,
    /// What the server has said about loading and indexing its project (#391).
    pub(crate) readiness: Arc<Readiness>,
    /// Documents retained by servers whose close/reopen lifecycle is not reliable, plus the
    /// URIs owned by each gateway session so an interrupted client can be cleaned up.
    pub(crate) documents: Mutex<DocumentLifecycle>,
    /// False once a retained-document generation is full. Replacing the whole engine is the
    /// only safe eviction for pyright: closing just one retained document revives #466.
    pub(crate) accepts_documents: Arc<AtomicBool>,
    pub(crate) ordinary_activity: Arc<AtomicUsize>,
    pub(crate) ordinary_epoch: Arc<AtomicU64>,
    pub(crate) probe_state: Arc<StdMutex<ProbeState>>,
    pub(crate) next_probe_id: Arc<AtomicU64>,
    pub(crate) health_probe_pending: Arc<StdMutex<Option<HealthProbePending>>>,
    pub(crate) _health_probe: Option<HealthProbeTask>,
    pub(crate) _child: Arc<StdMutex<Child>>,
    pub(crate) exit_details: Arc<StdMutex<Option<String>>>,
    pub(crate) stderr_tail: Arc<StdMutex<VecDeque<String>>>,
    pub(crate) stderr_done: tokio::sync::watch::Receiver<bool>,
}

impl GenericLspEngine {
    /// Spawn and initialize a generic language server for the workspace.
    pub async fn spawn(workspace_root: &Path, config: GenericLspConfig) -> Result<Self> {
        if config.health_probe_interval == Some(Duration::ZERO) {
            anyhow::bail!("language-server health probe interval must be greater than zero");
        }
        let health_probe_interval = config.health_probe_interval;
        let work_dir = config
            .working_dir
            .clone()
            .unwrap_or_else(|| workspace_root.to_path_buf());

        tracing::info!(
            workspace = ?workspace_root,
            command = %config.command,
            args = ?config.args,
            "Spawning generic LSP engine"
        );

        let mut cmd = Command::new(&config.command);
        cmd.kill_on_drop(true);
        cmd.args(&config.args)
            .current_dir(&work_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        for (k, v) in &config.env {
            cmd.env(k, v);
        }

        let mut attempts = 0;
        let mut child = loop {
            match cmd.spawn() {
                Ok(child) => break child,
                Err(err) if err.raw_os_error() == Some(26) && attempts < 10 => {
                    attempts += 1;
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Err(err) => {
                    return Err(err).with_context(|| {
                        format!(
                            "Failed to execute command: {} {:?}",
                            config.command, config.args
                        )
                    });
                }
            }
        };

        let stdin = child
            .stdin
            .take()
            .context("Failed to open child stdin for generic LSP")?;
        let stdout = child
            .stdout
            .take()
            .context("Failed to open child stdout for generic LSP")?;
        let stderr = child.stderr.take();
        let stderr_tail = Arc::new(StdMutex::new(VecDeque::<String>::with_capacity(32)));
        let stderr_tail_writer = Arc::clone(&stderr_tail);
        let (stderr_done_tx, stderr_done) = tokio::sync::watch::channel(false);
        if let Some(stderr) = stderr {
            spawn_stderr_reader(stderr, stderr_tail_writer, stderr_done_tx);
        } else {
            let _ = stderr_done_tx.send(true);
        }
        let exit_details = Arc::new(StdMutex::new(None::<String>));

        let (bcast_tx, _) = broadcast::channel(1024);
        let diagnostics: Arc<RwLock<HashMap<String, Published>>> =
            Arc::new(RwLock::new(HashMap::new()));
        let sent = Arc::new(RwLock::new(HashMap::<String, Sent>::new()));
        let versioned = Arc::new(AtomicBool::new(false));
        let apply_edit_waiter: Arc<Mutex<Option<oneshot::Sender<serde_json::Value>>>> =
            Arc::new(Mutex::new(None));

        let pending_requests: Arc<StdMutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>> =
            Arc::new(StdMutex::new(HashMap::new()));
        let health_probe_pending = Arc::new(StdMutex::new(None::<HealthProbePending>));
        let next_probe_id = Arc::new(AtomicU64::new(1));
        let probe_state = Arc::new(StdMutex::new(ProbeState::default()));
        let ordinary_activity = Arc::new(AtomicUsize::new(0));
        let ordinary_epoch = Arc::new(AtomicU64::new(0));

        let child = Arc::new(StdMutex::new(child));
        let stdin_arc = Arc::new(Mutex::new(stdin));
        let auto_reply_timeout = config.request_timeout;

        let is_alive = Arc::new(AtomicBool::new(true));
        let last_activity = Arc::new(RwLock::new(Instant::now()));

        let readiness = Arc::new(Readiness::new(config.ready));
        let capabilities = Arc::new(RwLock::new(None));
        let accepts_documents = Arc::new(AtomicBool::new(true));

        spawn_reader_loop(
            stdout,
            Arc::clone(&readiness),
            Arc::clone(&pending_requests),
            Arc::clone(&next_probe_id),
            Arc::clone(&health_probe_pending),
            Arc::clone(&probe_state),
            Arc::clone(&last_activity),
            workspace_root.to_path_buf(),
            Arc::clone(&apply_edit_waiter),
            Arc::clone(&stdin_arc),
            auto_reply_timeout,
            Arc::downgrade(&child),
            Arc::clone(&is_alive),
            Arc::clone(&sent),
            Arc::clone(&diagnostics),
            Arc::clone(&versioned),
            bcast_tx.clone(),
            Arc::clone(&accepts_documents),
            Arc::clone(&capabilities),
        );

        let mut engine = Self {
            workspace_root: workspace_root.to_path_buf(),
            config,
            stdin: stdin_arc,
            next_req_id: Arc::new(AtomicU64::new(1)),
            pending_requests,
            capabilities,
            broadcast_tx: bcast_tx,
            last_activity,
            diagnostics,
            sent,
            versioned,
            pull_unsupported: AtomicBool::new(false),
            apply_edit_waiter,
            is_alive,
            readiness,
            documents: Mutex::new(DocumentLifecycle::default()),
            accepts_documents,
            ordinary_activity,
            ordinary_epoch,
            probe_state,
            next_probe_id,
            health_probe_pending,
            _health_probe: None,
            _child: child,
            exit_details,
            stderr_tail,
            stderr_done,
        };

        // Initialize LSP server
        engine.initialize().await?;
        if let Some(interval) = health_probe_interval {
            engine.start_health_probe(interval);
        }

        Ok(engine)
    }

    /// Diagnostic explanation of why the language server exited, including exit status and stderr.
    pub async fn exit_details(&self) -> Option<String> {
        let current = lock_unpoisoned(&self.exit_details).clone();
        if current.is_some() {
            return current;
        }
        if self.is_alive.load(Ordering::Acquire) {
            return None;
        }
        let status_msg = {
            let mut child = lock_unpoisoned(&self._child);
            let Ok(Some(status)) = child.try_wait() else {
                return None;
            };
            match status.code() {
                Some(code) => format!("exit code {code}"),
                None => {
                    #[cfg(unix)]
                    {
                        use std::os::unix::process::ExitStatusExt;
                        status.signal().map_or_else(
                            || "process terminated".to_string(),
                            |sig| format!("signal {sig}"),
                        )
                    }
                    #[cfg(not(unix))]
                    {
                        "process terminated".to_string()
                    }
                }
            }
        };
        let mut stderr_done = self.stderr_done.clone();
        let drained = if *stderr_done.borrow() {
            true
        } else {
            tokio::time::timeout(Duration::from_secs(2), stderr_done.changed())
                .await
                .is_ok()
                && *stderr_done.borrow()
        };
        let tail = lock_unpoisoned(&self.stderr_tail);
        let details = if tail.is_empty() {
            if drained {
                status_msg
            } else {
                format!("{status_msg} (stderr drain timed out)")
            }
        } else {
            let lines: Vec<&str> = tail.iter().map(|s| s.as_str()).collect();
            if drained {
                format!("{status_msg} (stderr: {})", lines.join("\n"))
            } else {
                format!(
                    "{status_msg} (stderr [incomplete drain]: {})",
                    lines.join("\n")
                )
            }
        };
        if drained {
            *lock_unpoisoned(&self.exit_details) = Some(details.clone());
        }
        Some(details)
    }

    /// Check if the process is currently running and healthy.
    /// Whether the server process is still running: false once its output has ended, as it
    /// does when the server exits or crashes. The gateway loads a workspace afresh when its
    /// server has exited (#355).
    pub fn is_alive(&self) -> bool {
        self.is_alive.load(Ordering::Relaxed)
    }

    /// Elapsed time since the last active message.
    pub async fn idle_duration(&self) -> Duration {
        self.last_activity.read().await.elapsed()
    }

    /// Subscribe to background broadcast notifications.
    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.broadcast_tx.subscribe()
    }
}
