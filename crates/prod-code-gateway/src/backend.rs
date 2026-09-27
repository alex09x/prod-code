//! Managed language server backend workers (e.g. rust-analyzer on a Linux node) supervised by prod-code gateway.

use anyhow::{Context, Result};
use std::collections::{HashSet, VecDeque};
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, Weak};
use std::time::Duration;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, Notify, OwnedMutexGuard, RwLock, broadcast, oneshot};
use tokio::time::Instant;

const DEFAULT_WRITE_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_HEALTH_PROBE_INTERVAL: Duration = Duration::from_secs(60);
const MAX_IDLE_PROBE_TIMEOUTS: usize = 3;
const HEALTH_PROBE_METHOD: &str = "prodCode/healthProbe";
const HEALTH_PROBE_ID_PREFIX: &str = "prod-code-backend-health:";
const MAX_RETAINED_DISPATCH_IDENTITIES: usize = 256;
static NEXT_HEALTH_PROBE_NAMESPACE: AtomicU64 = AtomicU64::new(1);

fn lock_unpoisoned<T>(mutex: &StdMutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Default)]
struct ProbeState {
    consecutive_timeouts: usize,
    valid_evidence_epoch: u64,
    latest_valid_sequence: u64,
    valid_completions: u64,
}

struct HealthProbePending {
    id: String,
    response: oneshot::Sender<serde_json::Value>,
}

struct ProbePending {
    id: String,
    pending: Weak<StdMutex<Option<HealthProbePending>>>,
}

impl Drop for ProbePending {
    fn drop(&mut self) {
        if let Some(pending) = self.pending.upgrade() {
            let mut pending = lock_unpoisoned(&pending);
            if pending.as_ref().is_some_and(|slot| slot.id == self.id) {
                pending.take();
            }
        }
    }
}

fn health_probe_sequence(id: &serde_json::Value, id_prefix: &str) -> Option<u64> {
    let suffix = id.as_str()?.strip_prefix(id_prefix)?;
    let sequence = suffix.parse::<u64>().ok()?;
    (sequence != 0 && sequence.to_string() == suffix).then_some(sequence)
}

fn valid_dispatch_response(response: &serde_json::Value) -> bool {
    let Some(envelope) = response.as_object() else {
        return false;
    };
    if envelope.get("jsonrpc").and_then(serde_json::Value::as_str) != Some("2.0")
        || envelope.get("id").is_none_or(serde_json::Value::is_null)
        || envelope.contains_key("method")
    {
        return false;
    }
    match (envelope.get("result"), envelope.get("error")) {
        (Some(_), None) => true,
        (None, Some(error)) => {
            error
                .get("code")
                .and_then(serde_json::Value::as_i64)
                .is_some()
                && error
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .is_some()
        }
        _ => false,
    }
}

fn record_dispatch_liveness(
    ordinary_epoch: &AtomicU64,
    last_activity: &StdMutex<Instant>,
    probe_state: &StdMutex<ProbeState>,
) {
    ordinary_epoch.fetch_add(1, Ordering::AcqRel);
    *lock_unpoisoned(last_activity) = Instant::now();
    let mut state = lock_unpoisoned(probe_state);
    state.valid_evidence_epoch = state.valid_evidence_epoch.wrapping_add(1);
    state.consecutive_timeouts = 0;
}

async fn retire_health_generation(
    child: &Weak<StdMutex<Child>>,
    is_alive: &Weak<AtomicBool>,
    closed: &Weak<Notify>,
    capabilities: &Weak<RwLock<Option<serde_json::Value>>>,
    health_pending: &Weak<StdMutex<Option<HealthProbePending>>>,
) {
    if let Some(is_alive) = is_alive.upgrade() {
        is_alive.store(false, Ordering::Release);
    }
    if let Some(closed) = closed.upgrade() {
        closed.notify_waiters();
        closed.notify_one();
    }
    if let Some(child) = child.upgrade() {
        let _ = lock_unpoisoned(&child).start_kill();
    }
    if let Some(health_pending) = health_pending.upgrade() {
        lock_unpoisoned(&health_pending).take();
    }
    if let Some(capabilities) = capabilities.upgrade() {
        *capabilities.write().await = None;
    }
}

#[derive(Default)]
struct IssuedRequestHistory {
    next_token: u64,
    requests: VecDeque<(u64, serde_json::Value)>,
}

impl IssuedRequestHistory {
    fn insert(&mut self, id: serde_json::Value) -> u64 {
        self.next_token = self.next_token.wrapping_add(1);
        let token = self.next_token;
        self.requests.push_back((token, id));
        while self.requests.len() > MAX_RETAINED_DISPATCH_IDENTITIES {
            self.requests.pop_front();
        }
        token
    }

    fn remove(&mut self, token: u64) {
        if let Some(index) = self
            .requests
            .iter()
            .position(|(candidate, _)| *candidate == token)
        {
            self.requests.remove(index);
        }
    }

    fn contains(&self, id: &serde_json::Value) -> bool {
        self.requests.iter().any(|(_, candidate)| candidate == id)
    }
}

struct IssuedRequestRegistration {
    history: Weak<StdMutex<IssuedRequestHistory>>,
    token: u64,
    committed: bool,
}

impl Drop for IssuedRequestRegistration {
    fn drop(&mut self) {
        if !self.committed
            && let Some(history) = self.history.upgrade()
        {
            lock_unpoisoned(&history).remove(self.token);
        }
    }
}

struct OwnedTask(tokio::task::JoinHandle<()>);

impl Drop for OwnedTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[derive(Clone)]
struct FrameWriter {
    stdin: Arc<Mutex<ChildStdin>>,
    child: Weak<StdMutex<Child>>,
    is_alive: Arc<AtomicBool>,
    closed: Arc<Notify>,
    engine: Arc<str>,
    timeout: Duration,
    ordinary_epoch: Arc<AtomicU64>,
    last_activity: Arc<StdMutex<Instant>>,
    probe_state: Arc<StdMutex<ProbeState>>,
    issued_requests: Arc<StdMutex<IssuedRequestHistory>>,
    health_probe_id_prefix: Arc<str>,
}

impl FrameWriter {
    fn retire(&self) {
        self.is_alive.store(false, Ordering::Release);
        self.closed.notify_waiters();
        self.closed.notify_one();
        if let Some(child) = self.child.upgrade() {
            match child.lock() {
                Ok(mut child) => {
                    let _ = child.start_kill();
                }
                Err(error) => {
                    let mut child = error.into_inner();
                    let _ = child.start_kill();
                }
            }
        }
    }

    async fn send(&self, json_payload: &str) -> Result<()> {
        self.send_with_timeout(json_payload, self.timeout).await
    }

    async fn send_control(&self, json_payload: &str) -> Result<()> {
        self.write_frame_with_timeout(json_payload, self.timeout)
            .await
    }

    async fn send_with_timeout(&self, json_payload: &str, timeout: Duration) -> Result<()> {
        anyhow::ensure!(
            self.is_alive.load(Ordering::Acquire),
            "{} backend process has exited",
            self.engine
        );

        let ordinary_request = serde_json::from_str::<serde_json::Value>(json_payload)
            .ok()
            .filter(|value| {
                value.get("jsonrpc").and_then(serde_json::Value::as_str) == Some("2.0")
                    && value
                        .get("method")
                        .and_then(serde_json::Value::as_str)
                        .is_some()
                    && value.get("id").is_some_and(|id| !id.is_null())
            });
        if let Some(request) = &ordinary_request {
            anyhow::ensure!(
                health_probe_sequence(&request["id"], &self.health_probe_id_prefix).is_none(),
                "request id is reserved for private {} backend health probes",
                self.engine
            );
        }

        self.ordinary_epoch.fetch_add(1, Ordering::AcqRel);
        *lock_unpoisoned(&self.last_activity) = Instant::now();
        lock_unpoisoned(&self.probe_state).consecutive_timeouts = 0;
        let mut registration = ordinary_request.map(|value| {
            let token = lock_unpoisoned(&self.issued_requests).insert(value["id"].clone());
            IssuedRequestRegistration {
                history: Arc::downgrade(&self.issued_requests),
                token,
                committed: false,
            }
        });

        self.write_frame_with_timeout(json_payload, timeout).await?;
        if let Some(registration) = &mut registration {
            registration.committed = true;
        }
        Ok(())
    }

    async fn write_frame_with_timeout(&self, json_payload: &str, timeout: Duration) -> Result<()> {
        anyhow::ensure!(
            self.is_alive.load(Ordering::Acquire),
            "{} backend process has exited",
            self.engine
        );
        let deadline = Instant::now() + timeout;
        let stdin = tokio::time::timeout_at(deadline, Arc::clone(&self.stdin).lock_owned())
            .await
            .with_context(|| {
                format!(
                    "timed out after {timeout:?} waiting for {} backend writer",
                    self.engine
                )
            })?;
        anyhow::ensure!(
            self.is_alive.load(Ordering::Acquire),
            "{} backend process has exited",
            self.engine
        );

        let mut frame = FrameWrite::new(stdin, self.clone(), timeout);
        let header = format!("Content-Length: {}\r\n\r\n", json_payload.len());
        frame
            .write_all_until(deadline, header.as_bytes(), "header")
            .await?;
        frame
            .write_all_until(deadline, json_payload.as_bytes(), "payload")
            .await?;
        frame.flush_until(deadline).await?;
        frame.complete = true;
        Ok(())
    }
}

struct FrameWrite {
    stdin: OwnedMutexGuard<ChildStdin>,
    writer: FrameWriter,
    timeout: Duration,
    bytes_written: usize,
    faulted: bool,
    complete: bool,
}

impl FrameWrite {
    fn new(stdin: OwnedMutexGuard<ChildStdin>, writer: FrameWriter, timeout: Duration) -> Self {
        Self {
            stdin,
            writer,
            timeout,
            bytes_written: 0,
            faulted: false,
            complete: false,
        }
    }

    async fn write_all_until(
        &mut self,
        deadline: Instant,
        mut bytes: &[u8],
        part: &str,
    ) -> Result<()> {
        while !bytes.is_empty() {
            let written = match tokio::time::timeout_at(deadline, self.stdin.write(bytes)).await {
                Ok(Ok(0)) => {
                    self.faulted = true;
                    anyhow::bail!("{} backend writer returned zero bytes", self.writer.engine);
                }
                Ok(Ok(written)) => written,
                Ok(Err(error)) => {
                    self.faulted = true;
                    return Err(error).with_context(|| {
                        format!("failed writing {} backend LSP {part}", self.writer.engine)
                    });
                }
                Err(_) => anyhow::bail!(
                    "timed out after {:?} writing {} backend LSP {part}",
                    self.timeout,
                    self.writer.engine
                ),
            };
            self.bytes_written += written;
            bytes = &bytes[written..];
        }
        Ok(())
    }

    async fn flush_until(&mut self, deadline: Instant) -> Result<()> {
        match tokio::time::timeout_at(deadline, self.stdin.flush()).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => {
                self.faulted = true;
                Err(error).with_context(|| {
                    format!("failed flushing {} backend LSP frame", self.writer.engine)
                })
            }
            Err(_) => anyhow::bail!(
                "timed out after {:?} flushing {} backend LSP frame",
                self.timeout,
                self.writer.engine
            ),
        }
    }
}

impl Drop for FrameWrite {
    fn drop(&mut self) {
        if !self.complete && (self.bytes_written != 0 || self.faulted) {
            // Poison the connection before the owned mutex guard releases it: queued writers
            // must never append a new frame after an incomplete one.
            self.writer.retire();
        }
    }
}

/// Managed backend worker running a language server process on the host.
pub struct BackendWorker {
    pub engine: String,
    pub workspace_root: String,
    writer: FrameWriter,
    broadcast_tx: broadcast::Sender<String>,
    pub capabilities: Arc<RwLock<Option<serde_json::Value>>>,
    pub open_files: Arc<RwLock<HashSet<String>>>,
    is_alive: Arc<AtomicBool>,
    closed: Arc<Notify>,
    ordinary_epoch: Arc<AtomicU64>,
    last_activity: Arc<StdMutex<Instant>>,
    probe_state: Arc<StdMutex<ProbeState>>,
    next_probe_id: Arc<AtomicU64>,
    health_probe_pending: Arc<StdMutex<Option<HealthProbePending>>>,
    issued_requests: Arc<StdMutex<IssuedRequestHistory>>,
    reader_task: OwnedTask,
    health_task: Option<OwnedTask>,
    _child: Arc<StdMutex<Child>>,
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
        let tx_clone = tx.clone();
        let stdin_arc = Arc::new(Mutex::new(stdin));
        let is_alive = Arc::new(AtomicBool::new(true));
        let reader_alive = Arc::clone(&is_alive);
        let closed = Arc::new(Notify::new());
        let reader_closed = Arc::clone(&closed);
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
        let reader_child = Arc::downgrade(&child);
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
        let reader_writer = writer.clone();
        let reader_ordinary_epoch = Arc::clone(&ordinary_epoch);
        let reader_last_activity = Arc::clone(&last_activity);
        let reader_probe_state = Arc::clone(&probe_state);
        let reader_next_probe_id = Arc::clone(&next_probe_id);
        let reader_health_probe_id_prefix = Arc::clone(&health_probe_id_prefix);
        let reader_health_pending = Arc::clone(&health_probe_pending);
        let reader_issued_requests = Arc::clone(&issued_requests);

        // Background reader loop: reads Content-Length frames from language server stdout
        let reader_task = OwnedTask(tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            loop {
                let json = match prod_code_protocol::transport::read_lsp_frame(&mut reader).await {
                    Ok(Some(json)) => json,
                    Ok(None) => break,
                    Err(error) => {
                        tracing::warn!(%error, "Invalid backend LSP frame");
                        break;
                    }
                };
                // Auto-respond to server-initiated requests
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&json) {
                    let id = val.get("id");
                    let method = val.get("method").and_then(|m| m.as_str());
                    let probe_identity_sequence = id
                        .and_then(|id| health_probe_sequence(id, &reader_health_probe_id_prefix))
                        .filter(|sequence| {
                            *sequence < reader_next_probe_id.load(Ordering::Acquire)
                        });
                    let probe_sequence = method
                        .is_none()
                        .then_some(probe_identity_sequence)
                        .flatten();
                    if method.is_none()
                        && let (Some(id), Some(sequence)) =
                            (id.and_then(serde_json::Value::as_str), probe_sequence)
                    {
                        if valid_dispatch_response(&val) {
                            let response = {
                                let mut pending = lock_unpoisoned(&reader_health_pending);
                                if pending.as_ref().is_some_and(|slot| slot.id == id) {
                                    pending.take().map(|slot| slot.response)
                                } else {
                                    None
                                }
                            };
                            let mut state = lock_unpoisoned(&reader_probe_state);
                            state.valid_evidence_epoch = state.valid_evidence_epoch.wrapping_add(1);
                            state.consecutive_timeouts = 0;
                            if sequence > state.latest_valid_sequence {
                                state.latest_valid_sequence = sequence;
                                state.valid_completions += 1;
                            }
                            drop(state);
                            if let Some(response) = response {
                                let _ = response.send(val);
                            }
                        }
                        // Every issued private identity remains classifiable from this worker's
                        // private namespace after its one pending slot is cleared. Invalid and
                        // late probe replies therefore never leak into editor subscriptions.
                        continue;
                    }

                    if method.is_none() {
                        if valid_dispatch_response(&val)
                            && id.is_some_and(|id| {
                                lock_unpoisoned(&reader_issued_requests).contains(id)
                            })
                        {
                            record_dispatch_liveness(
                                &reader_ordinary_epoch,
                                &reader_last_activity,
                                &reader_probe_state,
                            );
                        }
                    } else if probe_identity_sequence.is_none()
                        && val.get("jsonrpc").and_then(serde_json::Value::as_str) == Some("2.0")
                    {
                        // Genuine server requests, progress and indexing notifications defer
                        // idle probes. A server request reusing a private probe id does not.
                        record_dispatch_liveness(
                            &reader_ordinary_epoch,
                            &reader_last_activity,
                            &reader_probe_state,
                        );
                    }
                    match (id, method) {
                        (
                            Some(id),
                            Some("window/workDoneProgress/create" | "client/registerCapability"),
                        ) => {
                            let auto_resp = serde_json::json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "result": null
                            })
                            .to_string();
                            if let Err(error) = reader_writer.send_control(&auto_resp).await {
                                tracing::warn!(%error, "Failed to write automatic backend LSP response");
                                // No caller can retry this mandatory response. Keeping a server
                                // waiting forever would leave a falsely reusable backend.
                                reader_writer.retire();
                                break;
                            }
                        }
                        (Some(id), Some("workspace/configuration")) => {
                            let items = val
                                .get("params")
                                .and_then(|params| params.get("items"))
                                .and_then(serde_json::Value::as_array)
                                .filter(|items| {
                                    items.iter().all(|item| {
                                        item.is_object()
                                            && ["section", "scopeUri"].iter().all(|key| {
                                                item.get(key).is_none_or(|value| value.is_string())
                                            })
                                    })
                                });
                            let auto_resp = match items {
                                Some(items) => serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": id,
                                    "result": vec![serde_json::json!({}); items.len()]
                                }),
                                None => serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": id,
                                    "error": {
                                        "code": -32602,
                                        "message": "workspace/configuration requires an items array of configuration objects with optional string section and scopeUri"
                                    }
                                }),
                            }
                            .to_string();
                            if let Err(error) = reader_writer.send_control(&auto_resp).await {
                                tracing::warn!(%error, "Failed to write automatic backend LSP response");
                                // No caller can retry this mandatory response. Keeping a server
                                // waiting forever would leave a falsely reusable backend.
                                reader_writer.retire();
                                break;
                            }
                        }
                        _ => {}
                    }
                    if probe_sequence.is_some() {
                        continue;
                    }
                }

                // Broadcast frame to all connected sessions
                let _ = tx_clone.send(json);
            }
            reader_alive.store(false, Ordering::Release);
            reader_closed.notify_waiters();
            reader_closed.notify_one();
            if let Some(child) = reader_child.upgrade() {
                match child.lock() {
                    Ok(mut child) => {
                        let _ = child.start_kill();
                    }
                    Err(error) => {
                        let mut child = error.into_inner();
                        let _ = child.start_kill();
                    }
                }
            }
            tracing::info!("Backend worker reader loop terminated");
        }));

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
            std::time::Duration::from_secs(10),
            worker.initialize_backend(workspace_root),
        )
        .await
        .context("Timeout initializing backend language server")?
        .context("Failed to initialize backend language server")?;

        worker.start_health_probe(health_probe_interval, health_response_timeout);

        Ok(worker)
    }

    fn start_health_probe(&mut self, interval: Duration, response_timeout: Duration) {
        let stdin = Arc::downgrade(&self.writer.stdin);
        let child = Arc::downgrade(&self._child);
        let is_alive = Arc::downgrade(&self.is_alive);
        let closed = Arc::downgrade(&self.closed);
        let capabilities = Arc::downgrade(&self.capabilities);
        let ordinary_epoch = Arc::downgrade(&self.ordinary_epoch);
        let last_activity = Arc::downgrade(&self.last_activity);
        let probe_state = Arc::downgrade(&self.probe_state);
        let next_probe_id = Arc::downgrade(&self.next_probe_id);
        let health_pending = Arc::downgrade(&self.health_probe_pending);
        let issued_requests = Arc::downgrade(&self.issued_requests);
        let health_probe_id_prefix = Arc::clone(&self.writer.health_probe_id_prefix);
        let engine = Arc::clone(&self.writer.engine);

        self.health_task = Some(OwnedTask(tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;

                let Some(alive) = is_alive.upgrade() else {
                    break;
                };
                if !alive.load(Ordering::Acquire) {
                    break;
                }
                let Some(epoch) = ordinary_epoch.upgrade() else {
                    break;
                };
                let Some(activity) = last_activity.upgrade() else {
                    break;
                };
                if lock_unpoisoned(&activity).elapsed() < interval {
                    continue;
                }
                let activity_before_wait = epoch.load(Ordering::Acquire);

                let Some(stdin) = stdin.upgrade() else {
                    break;
                };
                let Ok(stdin_guard) = Arc::clone(&stdin).try_lock_owned() else {
                    continue;
                };
                if !alive.load(Ordering::Acquire)
                    || epoch.load(Ordering::Acquire) != activity_before_wait
                    || lock_unpoisoned(&activity).elapsed() < interval
                {
                    continue;
                }

                let Some(state) = probe_state.upgrade() else {
                    break;
                };
                let evidence_before_wait = lock_unpoisoned(&state).valid_evidence_epoch;
                let Some(ids) = next_probe_id.upgrade() else {
                    break;
                };
                let sequence = ids.fetch_add(1, Ordering::AcqRel);
                let id = format!("{health_probe_id_prefix}{sequence}");
                let Some(pending_slot) = health_pending.upgrade() else {
                    break;
                };
                let (tx, rx) = oneshot::channel();
                *lock_unpoisoned(&pending_slot) = Some(HealthProbePending {
                    id: id.clone(),
                    response: tx,
                });
                let pending = ProbePending {
                    id: id.clone(),
                    pending: health_pending.clone(),
                };
                let Some(closed_notify) = closed.upgrade() else {
                    break;
                };
                let Some(history) = issued_requests.upgrade() else {
                    break;
                };
                let frame_writer = FrameWriter {
                    stdin: Arc::clone(&stdin),
                    child: child.clone(),
                    is_alive: Arc::clone(&alive),
                    closed: closed_notify,
                    engine: Arc::clone(&engine),
                    timeout: response_timeout,
                    ordinary_epoch: Arc::clone(&epoch),
                    last_activity: Arc::clone(&activity),
                    probe_state: Arc::clone(&state),
                    issued_requests: history,
                    health_probe_id_prefix: Arc::clone(&health_probe_id_prefix),
                };
                let payload = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": &id,
                    "method": HEALTH_PROBE_METHOD,
                    "params": {}
                })
                .to_string();
                let header = format!("Content-Length: {}\r\n\r\n", payload.len());
                let deadline = Instant::now() + response_timeout;
                let mut frame = FrameWrite::new(stdin_guard, frame_writer, response_timeout);
                let write_result = async {
                    frame
                        .write_all_until(deadline, header.as_bytes(), "health header")
                        .await?;
                    frame
                        .write_all_until(deadline, payload.as_bytes(), "health payload")
                        .await?;
                    frame.flush_until(deadline).await
                }
                .await;
                if let Err(error) = write_result {
                    if frame.bytes_written == 0 && !frame.faulted {
                        frame.complete = true;
                        tracing::debug!(%error, "backend health probe write made no progress");
                        continue;
                    }
                    tracing::warn!(%error, "backend health probe frame failed; retiring process");
                    drop(frame);
                    retire_health_generation(
                        &child,
                        &is_alive,
                        &closed,
                        &capabilities,
                        &health_pending,
                    )
                    .await;
                    break;
                }
                frame.complete = true;
                drop(frame);
                drop(stdin);
                drop(pending_slot);

                match tokio::time::timeout_at(deadline, rx).await {
                    Ok(Ok(_)) => {}
                    Ok(Err(_)) => break,
                    Err(_) => {
                        drop(pending);
                        let became_active = epoch.load(Ordering::Acquire) != activity_before_wait;
                        let should_retire = {
                            let mut state = lock_unpoisoned(&state);
                            if became_active || state.valid_evidence_epoch != evidence_before_wait {
                                state.consecutive_timeouts = 0;
                                false
                            } else {
                                state.consecutive_timeouts += 1;
                                state.consecutive_timeouts >= MAX_IDLE_PROBE_TIMEOUTS
                            }
                        };
                        if should_retire {
                            retire_health_generation(
                                &child,
                                &is_alive,
                                &closed,
                                &capabilities,
                                &health_pending,
                            )
                            .await;
                            break;
                        }
                    }
                }
            }
        })));
    }

    /// Perform the one-time LSP initialize handshake with the backend worker.
    async fn initialize_backend(&self, workspace_root: &Path) -> Result<()> {
        let ws_uri = prod_code_protocol::path::file_uri(workspace_root);
        let ws_name = workspace_root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("workspace");

        let init_req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "processId": null,
                "rootUri": ws_uri,
                "workspaceFolders": [
                    {
                        "name": ws_name,
                        "uri": ws_uri
                    }
                ],
                "capabilities": {
                    "workspace": {
                        "workspaceFolders": true,
                        "configuration": true
                    },
                    "textDocument": {
                        "hover": {
                            "contentFormat": ["markdown", "plaintext"]
                        },
                        "definition": {
                            "linkSupport": true
                        },
                        "documentSymbol": {
                            "hierarchicalDocumentSymbolSupport": true
                        },
                        "references": {}
                    }
                }
            }
        });

        let mut rx = self.subscribe();
        self.send_lsp(&init_req.to_string()).await?;

        // Server requests have their own IDs and may collide with our initialize request.
        // Only a response with the complete capabilities object establishes a usable server.
        loop {
            anyhow::ensure!(self.is_alive(), "backend exited during initialization");
            let message = tokio::select! {
                _ = self.closed.notified() => anyhow::bail!("backend exited during initialization"),
                message = rx.recv() => message.context("backend initialization response stream closed")?,
            };
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&message) else {
                continue;
            };
            if value.get("method").is_some()
                || value.get("id").and_then(serde_json::Value::as_u64) != Some(1)
            {
                continue;
            }
            if let Some(error) = value.get("error") {
                anyhow::bail!("backend refused initialization: {error}");
            }
            let capabilities = value
                .pointer("/result/capabilities")
                .filter(|capabilities| capabilities.is_object())
                .context("backend initialize response has no capabilities object")?;
            *self.capabilities.write().await = Some(capabilities.clone());
            break;
        }

        // Send initialized notification
        let initialized = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "initialized",
            "params": {}
        });
        self.send_lsp(&initialized.to_string()).await?;

        tracing::info!(workspace = %workspace_root.display(), "Backend language server initialized and warm");
        Ok(())
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
