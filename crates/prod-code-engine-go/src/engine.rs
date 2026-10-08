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
use prod_code_protocol::readiness::{Busy, Readiness, ReadySignal};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, RwLock, broadcast, oneshot};

use crate::config::{GoConfig, find_gopls_binary};
use crate::reader::spawn_reader_loop;
use crate::types::{HealthProbePending, HealthProbeTask, ProbeState, lock_unpoisoned};

/// Supervised Go engine managing an active `gopls` worker instance.
pub struct GoEngine {
    pub(crate) workspace_root: PathBuf,
    pub(crate) stdin: Arc<Mutex<ChildStdin>>,
    pub(crate) next_req_id: Arc<AtomicU64>,
    pub(crate) pending_requests: Arc<StdMutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>>,
    pub capabilities: Arc<RwLock<Option<serde_json::Value>>>,
    pub(crate) broadcast_tx: broadcast::Sender<String>,
    /// False once gopls's output has ended: it exited or crashed (#355).
    pub(crate) is_alive: Arc<AtomicBool>,
    /// What gopls has said about loading its packages: "Setting up workspace" is begun and
    /// ended as progress (#391).
    pub(crate) readiness: Arc<Readiness>,
    pub(crate) request_timeout: Duration,
    pub(crate) ordinary_activity: Arc<AtomicUsize>,
    pub(crate) ordinary_epoch: Arc<AtomicU64>,
    pub(crate) probe_state: Arc<StdMutex<ProbeState>>,
    pub(crate) next_probe_id: Arc<AtomicU64>,
    pub(crate) health_probe_pending: Arc<StdMutex<Option<HealthProbePending>>>,
    pub(crate) open_files: Arc<RwLock<HashMap<String, i32>>>,
    pub(crate) _health_probe: Option<HealthProbeTask>,
    pub(crate) _child: Arc<StdMutex<Child>>,
}

impl GoEngine {
    /// Launch a new managed Go engine for the given workspace root.
    pub async fn load(workspace_root: &Path, config: GoConfig) -> Result<Self> {
        Self::load_with_request_timeout(workspace_root, config, Duration::from_secs(30)).await
    }

    pub(crate) async fn load_with_request_timeout(
        workspace_root: &Path,
        config: GoConfig,
        request_timeout: Duration,
    ) -> Result<Self> {
        if config.health_probe_interval == Some(Duration::ZERO) {
            anyhow::bail!("gopls health probe interval must be greater than zero");
        }
        let health_probe_interval = config.health_probe_interval;
        let gopls_bin = find_gopls_binary(config.gopls_path.as_deref())
            .ok_or_else(|| anyhow::anyhow!("gopls executable not found on host"))?;

        // Determine shared NVMe cache directories
        let cache_base = config.shared_cache_dir.unwrap_or_else(|| {
            std::env::var("HOME")
                .map(|h| PathBuf::from(h).join(".cache/prod-code/go"))
                .unwrap_or_else(|_| std::env::temp_dir().join("prod-code-go-cache"))
        });
        let gocache = cache_base.join("gocache");
        let gomodcache = cache_base.join("modcache");

        tokio::fs::create_dir_all(&gocache)
            .await
            .context("Failed to create GOCACHE directory")?;
        tokio::fs::create_dir_all(&gomodcache)
            .await
            .context("Failed to create GOMODCACHE directory")?;

        tracing::info!(
            workspace = ?workspace_root,
            gopls = ?gopls_bin,
            ?gocache,
            ?gomodcache,
            "Spawning supervised gopls worker"
        );

        let mut cmd = Command::new(&gopls_bin);
        cmd.kill_on_drop(true);
        cmd.current_dir(workspace_root)
            .env("GOCACHE", &gocache)
            .env("GOMODCACHE", &gomodcache)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());

        for (k, v) in &config.extra_env {
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
                    return Err(err)
                        .with_context(|| format!("Failed to spawn gopls binary: {:?}", gopls_bin));
                }
            }
        };

        let stdin = child
            .stdin
            .take()
            .context("Failed to capture gopls child stdin")?;
        let stdout = child
            .stdout
            .take()
            .context("Failed to capture gopls child stdout")?;

        let (bcast_tx, _) = broadcast::channel(1024);
        let bcast_tx_clone = bcast_tx.clone();

        let pending_requests: Arc<StdMutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>> =
            Arc::new(StdMutex::new(HashMap::new()));
        let pending_clone = pending_requests.clone();
        let health_probe_pending = Arc::new(StdMutex::new(None::<HealthProbePending>));
        let health_probe_pending_reader = Arc::clone(&health_probe_pending);
        let next_probe_id = Arc::new(AtomicU64::new(1));
        let next_probe_id_reader = Arc::clone(&next_probe_id);
        let probe_state = Arc::new(StdMutex::new(ProbeState::default()));
        let probe_state_reader = Arc::clone(&probe_state);
        let ordinary_activity = Arc::new(AtomicUsize::new(0));
        let ordinary_epoch = Arc::new(AtomicU64::new(0));

        let child = Arc::new(StdMutex::new(child));
        let child_writer = Arc::downgrade(&child);
        let pending_writer = Arc::downgrade(&pending_requests);
        let stdin_arc = Arc::new(Mutex::new(stdin));
        let stdin_writer = stdin_arc.clone();
        let is_alive = Arc::new(AtomicBool::new(true));
        let is_alive_reader = Arc::clone(&is_alive);
        let readiness = Arc::new(Readiness::new(ReadySignal::Progress));
        let readiness_reader = Arc::clone(&readiness);
        let capabilities = Arc::new(RwLock::new(None));
        let capabilities_reader = Arc::clone(&capabilities);
        let auto_reply_timeout = request_timeout;

        spawn_reader_loop(
            stdout,
            readiness_reader,
            pending_clone,
            probe_state_reader,
            next_probe_id_reader,
            health_probe_pending_reader,
            stdin_writer,
            child_writer,
            pending_writer,
            is_alive_reader,
            capabilities_reader,
            bcast_tx_clone,
            auto_reply_timeout,
        );

        let mut engine = Self {
            workspace_root: workspace_root.to_path_buf(),
            stdin: stdin_arc,
            next_req_id: Arc::new(AtomicU64::new(1)),
            pending_requests,
            capabilities,
            broadcast_tx: bcast_tx,
            is_alive,
            readiness,
            request_timeout,
            ordinary_activity,
            ordinary_epoch,
            probe_state,
            next_probe_id,
            health_probe_pending,
            open_files: Arc::new(RwLock::new(HashMap::new())),
            _health_probe: None,
            _child: child,
        };

        // Initialize gopls with workspace root
        engine.initialize().await?;
        if let Some(interval) = health_probe_interval {
            engine.start_health_probe(interval);
        }

        Ok(engine)
    }

    pub(crate) fn retire_generation(&self) {
        self.is_alive.store(false, Ordering::Release);
        let _ = lock_unpoisoned(&self._child).start_kill();
        lock_unpoisoned(&self.pending_requests).clear();
        lock_unpoisoned(&self.health_probe_pending).take();
    }

    /// The package loading gopls is still doing, if any (#391).
    pub fn busy(&self) -> Option<Busy> {
        self.readiness.busy()
    }

    /// Whether gopls is still running: false once its output has ended (#355).
    pub fn is_alive(&self) -> bool {
        self.is_alive.load(Ordering::Relaxed)
    }

    /// Number of distinct scheduled health probes answered with a valid JSON-RPC envelope.
    #[doc(hidden)]
    pub fn health_probe_completions(&self) -> u64 {
        lock_unpoisoned(&self.probe_state).valid_completions
    }

    #[cfg(test)]
    pub(crate) fn retained_health_responses(&self) -> usize {
        usize::from(lock_unpoisoned(&self.health_probe_pending).is_some())
    }

    /// Subscribe to background server notifications.
    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.broadcast_tx.subscribe()
    }
}
