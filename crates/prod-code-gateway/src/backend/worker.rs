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
use prod_code_protocol::ScrubSecrets;
use std::collections::HashSet;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::{Mutex, Notify, RwLock, broadcast};
use tokio::time::Instant;

use super::probe::{
    DEFAULT_HEALTH_PROBE_INTERVAL, DEFAULT_WRITE_TIMEOUT, HEALTH_PROBE_ID_PREFIX,
    HealthProbePending, NEXT_HEALTH_PROBE_NAMESPACE, ProbeState, lock_unpoisoned,
};
use super::reader::spawn_reader_loop;
use super::request_history::{IssuedRequestHistory, OwnedTask};
use super::writer::FrameWriter;

/// Managed backend worker running a language server process on the host.
pub struct BackendWorker {
    pub engine: String,
    pub workspace_root: String,
    pub(crate) writer: FrameWriter,
    pub(crate) broadcast_tx: broadcast::Sender<String>,
    pub capabilities: Arc<RwLock<Option<serde_json::Value>>>,
    pub open_files: Arc<RwLock<HashSet<String>>>,
    pub(crate) is_alive: Arc<AtomicBool>,
    pub(crate) closed: Arc<Notify>,
    pub(crate) ordinary_epoch: Arc<AtomicU64>,
    pub(crate) last_activity: Arc<StdMutex<Instant>>,
    pub(crate) probe_state: Arc<StdMutex<ProbeState>>,
    pub(crate) next_probe_id: Arc<AtomicU64>,
    pub(crate) health_probe_pending: Arc<StdMutex<Option<HealthProbePending>>>,
    pub(crate) issued_requests: Arc<StdMutex<IssuedRequestHistory>>,
    pub(crate) reader_task: OwnedTask,
    pub(crate) health_task: Option<OwnedTask>,
    pub(crate) _child: Arc<StdMutex<tokio::process::Child>>,
}

impl BackendWorker {
    /// Spawn a language server worker for the specified workspace and initialize it.
    pub async fn spawn(workspace_root: &Path, engine: &str) -> Result<Self> {
        Self::spawn_with_health_config(
            workspace_root,
            engine,
            DEFAULT_WRITE_TIMEOUT,
            DEFAULT_HEALTH_PROBE_INTERVAL,
            DEFAULT_WRITE_TIMEOUT,
        )
        .await
    }

    #[doc(hidden)]
    pub async fn spawn_with_write_timeout(
        workspace_root: &Path,
        engine: &str,
        write_timeout: Duration,
    ) -> Result<Self> {
        Self::spawn_with_health_config(
            workspace_root,
            engine,
            write_timeout,
            DEFAULT_HEALTH_PROBE_INTERVAL,
            write_timeout,
        )
        .await
    }

    #[doc(hidden)]
    pub async fn spawn_with_health_config(
        workspace_root: &Path,
        engine: &str,
        write_timeout: Duration,
        health_probe_interval: Duration,
        health_response_timeout: Duration,
    ) -> Result<Self> {
        anyhow::ensure!(
            !health_probe_interval.is_zero(),
            "backend health probe interval must be greater than zero"
        );
        anyhow::ensure!(
            !health_response_timeout.is_zero(),
            "backend health response timeout must be greater than zero"
        );
        let binary = match engine {
            "rust" => "rust-analyzer",
            "go" => "gopls",
            other => anyhow::bail!("no managed backend language server for engine `{other}`"),
        };

        tracing::info!(
            engine,
            binary,
            ?workspace_root,
            "Spawning backend language server"
        );

        let mut cmd = Command::new(binary);
        cmd.kill_on_drop(true);
        cmd.current_dir(workspace_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        // The cluster's secret credentials (token and TLS material) are never given to backend language servers (#402, Phase 5.6).
        cmd.scrub_cluster_secrets();

        let mut child = cmd
            .spawn()
            .with_context(|| format!("Failed to spawn {binary} at {:?}", workspace_root))?;

        let stdin = child
            .stdin
            .take()
            .context("Failed to open stdin for backend language server")?;
        let stdout = child
            .stdout
            .take()
            .context("Failed to open stdout for backend language server")?;

        let (tx, _rx) = broadcast::channel::<String>(2048);
        let stdin_arc = Arc::new(Mutex::new(stdin));
        let is_alive = Arc::new(AtomicBool::new(true));
        let closed = Arc::new(Notify::new());
        let child = Arc::new(StdMutex::new(child));
        let ordinary_epoch = Arc::new(AtomicU64::new(0));
        let last_activity = Arc::new(StdMutex::new(Instant::now()));
        let probe_state = Arc::new(StdMutex::new(ProbeState::default()));
        let next_probe_id = Arc::new(AtomicU64::new(1));
        let health_probe_id_prefix: Arc<str> = Arc::from(format!(
            "{HEALTH_PROBE_ID_PREFIX}{}:",
            NEXT_HEALTH_PROBE_NAMESPACE.fetch_add(1, Ordering::Relaxed)
        ));
        let health_probe_pending: Arc<StdMutex<Option<HealthProbePending>>> =
            Arc::new(StdMutex::new(None));
        let issued_requests = Arc::new(StdMutex::new(IssuedRequestHistory::default()));
        let writer = FrameWriter {
            stdin: Arc::clone(&stdin_arc),
            child: Arc::downgrade(&child),
            is_alive: Arc::clone(&is_alive),
            closed: Arc::clone(&closed),
            engine: Arc::from(engine),
            timeout: write_timeout,
            ordinary_epoch: Arc::clone(&ordinary_epoch),
            last_activity: Arc::clone(&last_activity),
            probe_state: Arc::clone(&probe_state),
            issued_requests: Arc::clone(&issued_requests),
            health_probe_id_prefix: Arc::clone(&health_probe_id_prefix),
        };

        let reader_task = spawn_reader_loop(
            stdout,
            tx.clone(),
            Arc::clone(&is_alive),
            Arc::clone(&closed),
            Arc::downgrade(&child),
            Arc::clone(&ordinary_epoch),
            Arc::clone(&last_activity),
            Arc::clone(&probe_state),
            Arc::clone(&next_probe_id),
            Arc::clone(&health_probe_id_prefix),
            Arc::clone(&health_probe_pending),
            Arc::clone(&issued_requests),
            writer.clone(),
        );

        let mut worker = Self {
            engine: engine.to_string(),
            workspace_root: workspace_root.to_string_lossy().to_string(),
            writer,
            broadcast_tx: tx,
            capabilities: Arc::new(RwLock::new(None)),
            open_files: Arc::new(RwLock::new(HashSet::new())),
            is_alive,
            closed,
            ordinary_epoch,
            last_activity,
            probe_state,
            next_probe_id,
            health_probe_pending,
            issued_requests,
            reader_task,
            health_task: None,
            _child: child,
        };

        // Perform backend initialization handshake so the backend is warm and ready
        tokio::time::timeout(
            Duration::from_secs(10),
            worker.initialize_backend(workspace_root),
        )
        .await
        .context("Timeout initializing backend language server")?
        .context("Failed to initialize backend language server")?;

        worker.start_health_probe(health_probe_interval, health_response_timeout);

        Ok(worker)
    }

    /// False after the managed language server's output has ended.
    pub fn is_alive(&self) -> bool {
        self.is_alive.load(Ordering::Acquire)
    }

    #[doc(hidden)]
    pub fn health_probe_completions(&self) -> u64 {
        lock_unpoisoned(&self.probe_state).valid_completions
    }

    #[doc(hidden)]
    pub fn retained_health_responses(&self) -> usize {
        usize::from(lock_unpoisoned(&self.health_probe_pending).is_some())
    }

    #[doc(hidden)]
    pub fn retained_dispatch_identities(&self) -> usize {
        lock_unpoisoned(&self.issued_requests).requests.len()
    }

    #[doc(hidden)]
    pub fn process_id(&self) -> Option<u32> {
        lock_unpoisoned(&self._child).id()
    }

    /// Subscribe to raw LSP frames produced by this backend worker.
    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.broadcast_tx.subscribe()
    }

    /// Send an LSP JSON-RPC message into the backend language server's stdin.
    pub async fn send_lsp(&self, json_payload: &str) -> Result<()> {
        self.writer.send(json_payload).await
    }

    #[doc(hidden)]
    pub async fn send_lsp_with_write_timeout(
        &self,
        json_payload: &str,
        write_timeout: Duration,
    ) -> Result<()> {
        self.writer
            .send_with_timeout(json_payload, write_timeout)
            .await
    }
}

impl Drop for BackendWorker {
    fn drop(&mut self) {
        if let Some(task) = self.health_task.take() {
            drop(task);
        }
        self.reader_task.0.abort();
        self.writer.retire();
    }
}
