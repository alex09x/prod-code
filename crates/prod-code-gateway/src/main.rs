//! prod-code gateway daemon: multi-tenant server for remote code intelligence over 10 GbE LAN.

pub mod backend;
pub mod detect;
pub mod memory;
pub mod workspace;

pub use detect::detect_engine;

use anyhow::Result;
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{
    ExecChanges, ExecChunk, ExecExit, ExecRequest, FileDelta, FileStamp, HandshakeResponse,
    PROTOCOL_VERSION, PathTranslator, ProdCodeCodec, StatusResponse, SyncProbeRequest,
    SyncProbeResponse, SyncRequest, SyncResponse, WireMessage, content_hash,
};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;
use tokio::net::{TcpListener, TcpStream};
use tokio_util::codec::Framed;
use workspace::{SessionView, WorkspaceManager};

pub static NEXT_REQ_ID: AtomicU64 = AtomicU64::new(1);
pub static ACTIVE_QUERIES: AtomicUsize = AtomicUsize::new(0);
pub static TOTAL_QUERIES: AtomicU64 = AtomicU64::new(0);
pub static SLOW_QUERIES: AtomicU64 = AtomicU64::new(0);

#[derive(Parser, Debug)]
#[command(
    name = "prod-code-server",
    author,
    version,
    about = "Remote Code Intelligence Gateway"
)]
pub struct ServerCli {
    /// Bind address (IP:port). Defaults to 0.0.0.0:9400.
    #[arg(short, long, env = "PROD_CODE_BIND", default_value = "0.0.0.0:9400")]
    pub bind: SocketAddr,

    /// Workspace root storage directory on server.
    #[arg(
        short,
        long,
        env = "PROD_CODE_STORAGE",
        default_value = "/srv/prod-code/workspaces"
    )]
    pub storage: PathBuf,

    /// Unload a workspace's engine after this many seconds without a session (0 disables).
    #[arg(long, env = "PROD_CODE_IDLE_EVICT_SECS", default_value_t = 1800)]
    pub idle_evict_secs: u64,

    /// Delete `<repo>--wt-*` workspace directories unused for this many days (0 disables).
    #[arg(long, env = "PROD_CODE_PRUNE_WORKTREE_DAYS", default_value_t = 7)]
    pub prune_worktree_days: u64,
}

pub struct ServerState {
    pub start_time: Instant,
    pub server_pid: u32,
    pub next_session_id: AtomicU64,
    pub active_sessions: AtomicUsize,
    pub storage_root: PathBuf,
    pub workspace_manager: Arc<WorkspaceManager>,
}

impl ServerState {
    pub fn new(storage_root: PathBuf) -> Self {
        Self {
            start_time: Instant::now(),
            server_pid: std::process::id(),
            next_session_id: AtomicU64::new(1),
            active_sessions: AtomicUsize::new(0),
            storage_root,
            workspace_manager: Arc::new(WorkspaceManager::new()),
        }
    }

    pub async fn status(&self) -> StatusResponse {
        StatusResponse {
            server_pid: self.server_pid,
            uptime_seconds: self.start_time.elapsed().as_secs(),
            active_sessions: self.active_sessions.load(Ordering::Relaxed),
            loaded_workspaces: self.workspace_manager.loaded_count().await,
            detected_engines: vec![
                "rust (ra_ap_ide)".to_string(),
                "go (gopls)".to_string(),
                "generic-lsp".to_string(),
            ],
            memory_rss_bytes: memory::get_process_rss_bytes(),
            total_queries: TOTAL_QUERIES.load(Ordering::Relaxed),
            active_queries: ACTIVE_QUERIES.load(Ordering::Relaxed),
        }
    }
}

fn copy_tree(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<usize> {
    let mut copied = 0;
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str == ".git"
            || name_str == "target"
            || name_str == "node_modules"
            || name_str == workspace::LAST_USED_MARKER
        {
            continue;
        }
        let from = entry.path();
        let to = dst.join(&name);
        if from.is_dir() {
            copied += copy_tree(&from, &to)?;
        } else if from.is_file() {
            std::fs::copy(&from, &to)?;
            copied += 1;
        }
    }
    Ok(copied)
}

fn walk_files(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name == ".git"
            || name == "target"
            || name == "node_modules"
            || name == workspace::LAST_USED_MARKER
        {
            continue;
        }
        if path.is_dir() {
            walk_files(root, &path, out);
        } else if path.is_file()
            && let Ok(rel) = path.strip_prefix(root)
        {
            let rel = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join("/");
            out.push((rel, path));
        }
    }
}

/// Compares the workspace directory with the client's manifest: deletes files the client does
/// not have, and returns `(missing, deleted)` where `missing` are manifest paths the server
/// lacks or holds with different content.
fn reconcile_manifest(root: &std::path::Path, stamps: &[FileStamp]) -> (Vec<String>, Vec<String>) {
    let wanted: std::collections::HashMap<&str, &FileStamp> = stamps
        .iter()
        .map(|s| (s.relative_path.as_str(), s))
        .collect();
    let mut present = Vec::new();
    walk_files(root, root, &mut present);
    let mut missing = Vec::new();
    let mut deleted = Vec::new();
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for (rel, path) in &present {
        match wanted.get(rel.as_str()) {
            None => {
                if std::fs::remove_file(path).is_ok() {
                    deleted.push(rel.clone());
                }
            }
            Some(stamp) => {
                seen.insert(stamp.relative_path.as_str());
                let same = std::fs::metadata(path)
                    .map(|m| m.len() == stamp.size)
                    .unwrap_or(false)
                    && std::fs::read(path)
                        .map(|bytes| content_hash(&bytes) == stamp.hash)
                        .unwrap_or(false);
                if !same {
                    missing.push(rel.clone());
                }
            }
        }
    }
    for stamp in stamps {
        if !seen.contains(stamp.relative_path.as_str()) {
            missing.push(stamp.relative_path.clone());
        }
    }
    missing.sort();
    missing.dedup();
    (missing, deleted)
}

/// Answers a manifest probe: seeds a fresh workspace from the origin repository's copy,
/// reconciles it with the client's manifest, and reports what the client must still upload.
pub async fn apply_sync_probe(
    storage_root: &std::path::Path,
    workspace_manager: &WorkspaceManager,
    req: SyncProbeRequest,
) -> SyncProbeResponse {
    let start = Instant::now();
    let target = workspace::server_workspace_path(
        storage_root,
        &req.client_workspace_root,
        req.base_workspace_name.as_deref(),
    );
    let fresh = !target.exists()
        || std::fs::read_dir(&target)
            .map(|mut d| d.next().is_none())
            .unwrap_or(true);
    let mut seeded = false;
    if fresh && let Some(seed) = req.seed_from.as_deref() {
        let seed_dir = storage_root.join(workspace::sanitize_identifier(seed.trim()));
        if seed_dir.is_dir() && seed_dir != target {
            let (from, to) = (seed_dir.clone(), target.clone());
            match tokio::task::spawn_blocking(move || copy_tree(&from, &to)).await {
                Ok(Ok(files)) => {
                    seeded = true;
                    tracing::info!(
                        workspace = %target.display(),
                        seed = %seed_dir.display(),
                        files,
                        "🌱 [SEED] new worktree workspace seeded from origin copy"
                    );
                }
                Ok(Err(e)) => {
                    tracing::warn!(error = %e, seed = %seed_dir.display(), "seeding failed")
                }
                Err(e) => tracing::warn!(error = %e, "seeding task failed"),
            }
        }
    }
    let _ = std::fs::create_dir_all(&target);

    let root = target.clone();
    let stamps = req.files;
    let manifest_len = stamps.len();
    let (missing, deleted) =
        tokio::task::spawn_blocking(move || reconcile_manifest(&root, &stamps))
            .await
            .unwrap_or_default();

    if !deleted.is_empty()
        && let Some(engine_lock) = workspace_manager
            .get_loaded(&target)
            .await
            .and_then(|ws| ws.rust_engine.clone())
    {
        let mut engine = engine_lock.lock().await;
        for rel in &deleted {
            let _ = engine.update_base(&target.join(rel), None);
        }
    }

    tracing::info!(
        workspace = %target.display(),
        manifest = manifest_len,
        missing = missing.len(),
        deleted = deleted.len(),
        seeded,
        duration_ms = %format!("{}ms", start.elapsed().as_millis()),
        "🔎 [PROBE] manifest reconciled"
    );

    SyncProbeResponse {
        server_workspace_root: target.to_string_lossy().to_string(),
        seeded,
        files_deleted: deleted.len(),
        missing,
    }
}

/// Default wall-clock limit for a remote command when the client does not set one.
const EXEC_DEFAULT_TIMEOUT_SECS: u64 = 3600;

/// Size and content hash of every file under `root` the sync layer cares about (build output
/// and VCS internals excluded), for detecting what a remote command changed.
fn snapshot_tree(root: &std::path::Path) -> std::collections::HashMap<String, (u64, u64)> {
    let mut present = Vec::new();
    walk_files(root, root, &mut present);
    present
        .into_iter()
        .filter_map(|(rel, path)| {
            let bytes = std::fs::read(&path).ok()?;
            Some((rel, (bytes.len() as u64, content_hash(&bytes))))
        })
        .collect()
}

/// Files that differ between `before` and the tree now: new/changed ones with content,
/// removed ones as deletions. Files above 5 MiB are ignored.
fn changed_since(
    root: &std::path::Path,
    before: &std::collections::HashMap<String, (u64, u64)>,
) -> Vec<FileDelta> {
    const MAX_PULL_FILE: u64 = 5 * 1024 * 1024;
    let after = snapshot_tree(root);
    let mut out = Vec::new();
    for (rel, stamp) in &after {
        if before.get(rel) == Some(stamp) || stamp.0 > MAX_PULL_FILE {
            continue;
        }
        if let Ok(content) = std::fs::read(root.join(rel)) {
            #[cfg(unix)]
            let is_executable = {
                use std::os::unix::fs::PermissionsExt;
                std::fs::metadata(root.join(rel))
                    .map(|m| m.permissions().mode() & 0o111 != 0)
                    .unwrap_or(false)
            };
            #[cfg(not(unix))]
            let is_executable = false;
            out.push(FileDelta {
                relative_path: rel.clone(),
                content: Some(content),
                is_executable,
            });
        }
    }
    for rel in before.keys() {
        if !after.contains_key(rel) {
            out.push(FileDelta {
                relative_path: rel.clone(),
                content: None,
                is_executable: false,
            });
        }
    }
    out.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    out
}

/// Kills the command and everything it spawned (its process group), then the child itself.
fn kill_exec_tree(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        let _ = std::process::Command::new("kill")
            .args(["-9", "--", &format!("-{pid}")])
            .status();
    }
    let _ = child.start_kill();
}

/// Runs `req.command` inside the client's server workspace, streaming stdout/stderr chunks to
/// the client and finishing with an `ExecExit`. The child is killed if the client goes away
/// or the timeout elapses.
pub async fn run_exec(
    storage_root: &std::path::Path,
    framed: &mut Framed<TcpStream, ProdCodeCodec>,
    req: ExecRequest,
) -> Result<()> {
    use tokio::io::AsyncReadExt;

    let start = Instant::now();
    let workspace = workspace::server_workspace_path(
        storage_root,
        &req.client_workspace_root,
        req.base_workspace_name.as_deref(),
    );
    let workspace_str = workspace.to_string_lossy().to_string();
    let fail = |error: String| ExecExit {
        exit_code: None,
        duration_ms: 0,
        server_workspace_root: workspace_str.clone(),
        timed_out: false,
        error: Some(error),
    };
    if !workspace.is_dir() {
        framed
            .send(WireMessage::ExecExit(fail(format!(
                "workspace {workspace_str} is not synced to this gateway"
            ))))
            .await?;
        return Ok(());
    }
    let Some((program, args)) = req.command.split_first() else {
        framed
            .send(WireMessage::ExecExit(fail("empty command".to_string())))
            .await?;
        return Ok(());
    };
    workspace::touch_last_used(&workspace);
    let before = if req.pull_changes {
        let root = workspace.clone();
        tokio::task::spawn_blocking(move || snapshot_tree(&root))
            .await
            .unwrap_or_default()
    } else {
        Default::default()
    };

    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args)
        .current_dir(&workspace)
        .envs(req.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    // Own process group, so a timeout or client disconnect can take down the whole tree
    // (cargo -> test binary -> its helpers), not just the direct child.
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            framed
                .send(WireMessage::ExecExit(fail(format!(
                    "failed to start {program}: {e}"
                ))))
                .await?;
            return Ok(());
        }
    };
    tracing::info!(
        workspace = %workspace_str,
        command = %req.command.join(" "),
        "🛠️ [EXEC] started"
    );

    let (tx, mut rx) = tokio::sync::mpsc::channel::<ExecChunk>(256);
    let mut readers = Vec::new();
    if let Some(mut out) = child.stdout.take() {
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
    if let Some(mut err) = child.stderr.take() {
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
    drop(tx);

    let timeout = std::time::Duration::from_secs(if req.timeout_secs == 0 {
        EXEC_DEFAULT_TIMEOUT_SECS
    } else {
        req.timeout_secs
    });
    let deadline = tokio::time::Instant::now() + timeout;
    let mut timed_out = false;
    let mut status = None;
    let mut chunks_open = true;
    loop {
        tokio::select! {
            chunk = rx.recv(), if chunks_open => match chunk {
                Some(chunk) => framed.send(WireMessage::ExecChunk(chunk)).await?,
                None => chunks_open = false,
            },
            exit = child.wait(), if status.is_none() => {
                status = Some(exit);
            }
            _ = tokio::time::sleep_until(deadline), if !timed_out => {
                timed_out = true;
                kill_exec_tree(&mut child);
            }
            incoming = framed.next(), if status.is_none() => match incoming {
                Some(Ok(WireMessage::Ping)) => framed.send(WireMessage::Pong).await?,
                Some(Ok(WireMessage::Disconnect { .. })) | None => {
                    kill_exec_tree(&mut child);
                    tracing::info!(workspace = %workspace_str, "🛠️ [EXEC] client left; command killed");
                    return Ok(());
                }
                _ => {}
            }
        }
        if !chunks_open && status.is_some() {
            break;
        }
    }
    for reader in readers {
        let _ = reader.await;
    }
    let exit_code = status.and_then(|s| s.ok()).and_then(|s| s.code());
    let duration_ms = start.elapsed().as_millis() as u64;
    tracing::info!(
        workspace = %workspace_str,
        command = %req.command.join(" "),
        exit_code = ?exit_code,
        timed_out,
        duration_ms,
        "🛠️ [EXEC] finished"
    );
    if req.pull_changes {
        let root = workspace.clone();
        let files = tokio::task::spawn_blocking(move || changed_since(&root, &before))
            .await
            .unwrap_or_default();
        if !files.is_empty() {
            tracing::info!(
                workspace = %workspace_str,
                files = files.len(),
                "🛠️ [EXEC] sending back files the command changed"
            );
            framed
                .send(WireMessage::ExecChanges(ExecChanges { files }))
                .await?;
        }
    }
    framed
        .send(WireMessage::ExecExit(ExecExit {
            exit_code,
            duration_ms,
            server_workspace_root: workspace_str,
            timed_out,
            error: None,
        }))
        .await?;
    Ok(())
}

/// Apply batch file synchronization to server workspace storage.
pub async fn apply_sync(
    storage_root: &std::path::Path,
    workspace_manager: &WorkspaceManager,
    req: SyncRequest,
) -> SyncResponse {
    let start = Instant::now();
    let server_workspace = workspace::resolve_server_workspace(
        storage_root,
        &req.client_workspace_root,
        req.base_workspace_name.as_deref(),
    );
    let folder_name = server_workspace
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("default");
    // A workspace that is already warm in RAM must see the synced files as its new base.
    let loaded_rust = workspace_manager
        .get_loaded(&server_workspace)
        .await
        .and_then(|ws| ws.rust_engine.clone());

    let mut files_updated = 0;
    let mut files_deleted = 0;
    let mut bytes_transferred = 0;

    for delta in req.files {
        let target_path = server_workspace.join(&delta.relative_path);
        match delta.content {
            Some(content_bytes) => {
                if let Some(parent) = target_path.parent() {
                    let _ = tokio::fs::create_dir_all(parent).await;
                }
                bytes_transferred += content_bytes.len();
                if tokio::fs::write(&target_path, &content_bytes).await.is_ok() {
                    files_updated += 1;
                }
                if let (Some(engine_lock), Ok(text)) =
                    (&loaded_rust, std::str::from_utf8(&content_bytes))
                {
                    let mut engine = engine_lock.lock().await;
                    if let Err(e) = engine.update_base(&target_path, Some(text.to_string())) {
                        tracing::warn!(error = %e, file = %target_path.display(), "base update failed");
                    }
                }
            }
            None => {
                if target_path.exists() && tokio::fs::remove_file(&target_path).await.is_ok() {
                    files_deleted += 1;
                }
                if let Some(engine_lock) = &loaded_rust {
                    let mut engine = engine_lock.lock().await;
                    if let Err(e) = engine.update_base(&target_path, None) {
                        tracing::warn!(error = %e, file = %target_path.display(), "base removal failed");
                    }
                }
            }
        }
    }

    let duration_ms = start.elapsed().as_millis() as u64;

    tracing::info!(
        folder_name,
        files_updated,
        files_deleted,
        bytes_transferred,
        duration_ms = %format!("{duration_ms}ms"),
        "⚡ [SYNC] Workspace fast-sync applied"
    );

    SyncResponse {
        files_updated,
        files_deleted,
        bytes_transferred,
        duration_ms,
        server_workspace_root: server_workspace.to_string_lossy().to_string(),
    }
}

pub async fn handle_client(
    socket: TcpStream,
    addr: SocketAddr,
    state: Arc<ServerState>,
) -> Result<()> {
    let mut framed = Framed::new(socket, ProdCodeCodec::new());

    while let Some(msg_res) = framed.next().await {
        let msg = msg_res?;
        match msg {
            WireMessage::StatusRequest => {
                let status = state.status().await;
                framed.send(WireMessage::StatusResponse(status)).await?;
            }
            WireMessage::SyncRequest(req) => {
                let resp = apply_sync(&state.storage_root, &state.workspace_manager, req).await;
                framed.send(WireMessage::SyncResponse(resp)).await?;
            }
            WireMessage::SyncProbeRequest(req) => {
                let resp =
                    apply_sync_probe(&state.storage_root, &state.workspace_manager, req).await;
                framed.send(WireMessage::SyncProbeResponse(resp)).await?;
            }
            WireMessage::ExecRequest(req) => {
                run_exec(&state.storage_root, &mut framed, req).await?;
            }
            WireMessage::Ping => {
                framed.send(WireMessage::Pong).await?;
            }
            WireMessage::HandshakeRequest(req) => {
                let session_id = state.next_session_id.fetch_add(1, Ordering::Relaxed);
                state.active_sessions.fetch_add(1, Ordering::Relaxed);

                let client_root_path = PathBuf::from(&req.client_workspace_root);
                let server_workspace = workspace::resolve_server_workspace(
                    &state.storage_root,
                    &req.client_workspace_root,
                    req.base_workspace_name.as_deref(),
                );
                let server_workspace_str = server_workspace.to_string_lossy().to_string();
                workspace::touch_last_used(&server_workspace);

                let engine_kind =
                    detect::resolve_engine(&server_workspace, req.preferred_engine.as_deref());
                let engine = engine_kind.as_str();
                let translator =
                    PathTranslator::new(&req.client_workspace_root, &server_workspace_str);

                // Attach to shared workspace using leader-follower coalescing
                let shared_ws = state
                    .workspace_manager
                    .get_or_load(&server_workspace, engine)
                    .await?;

                let session_view = state
                    .workspace_manager
                    .register_session_view(session_id, client_root_path.clone(), shared_ws)
                    .await;

                tracing::info!(
                    session_id,
                    client_pid = req.client_pid,
                    client_root = %req.client_workspace_root,
                    server_root = %server_workspace_str,
                    engine,
                    is_single_owner = session_view.is_single_owner,
                    "Client session established (Direct-Edit fast path active: {})",
                    session_view.is_single_owner
                );

                framed
                    .send(WireMessage::HandshakeResponse(HandshakeResponse {
                        protocol_version: PROTOCOL_VERSION,
                        server_pid: state.server_pid,
                        session_id,
                        server_workspace_root: server_workspace_str,
                        detected_engine: engine.to_string(),
                    }))
                    .await?;

                // Session loop for streaming LSP and control messages
                let session_res = run_session_loop(framed, &translator, &session_view).await;

                if let Some(engine_lock) = &session_view.workspace.rust_engine {
                    let mut engine = engine_lock.lock().await;
                    if let Err(e) = engine.clear_session(session_id) {
                        tracing::warn!(error = %e, session_id, "failed to drop session overlays");
                    }
                }

                state
                    .workspace_manager
                    .unregister_session_view(&session_view)
                    .await;
                state.active_sessions.fetch_sub(1, Ordering::Relaxed);

                tracing::info!(session_id, "Client session retired: {:?}", session_res);
                return session_res;
            }
            WireMessage::Disconnect { reason } => {
                tracing::info!(%addr, reason, "Client disconnected cleanly");
                break;
            }
            other => {
                tracing::warn!(%addr, ?other, "Unexpected message before handshake");
            }
        }
    }

    Ok(())
}

async fn run_session_loop(
    framed: Framed<TcpStream, ProdCodeCodec>,
    translator: &PathTranslator,
    view: &SessionView,
) -> Result<()> {
    let (mut socket_tx, mut socket_rx) = framed.split();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<WireMessage>(4096);

    let writer_handle = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            if socket_tx.send(msg).await.is_err() {
                break;
            }
        }
    });

    let mut backend_rx = if let Some(ref go) = view.workspace.go_engine {
        Some(go.subscribe())
    } else if let Some(ref generic_eng) = view.workspace.generic_engine {
        Some(generic_eng.subscribe())
    } else {
        view.workspace.backend.as_ref().map(|b| b.subscribe())
    };

    loop {
        tokio::select! {
            client_msg_res = socket_rx.next() => {
                match client_msg_res {
                    Some(Ok(WireMessage::Ping)) => {
                        let _ = out_tx.send(WireMessage::Pong).await;
                    }
                    Some(Ok(WireMessage::LspPayload(raw_client_lsp))) => {
                        let server_lsp = translator.translate_lsp_to_server(&raw_client_lsp);
                        tracing::debug!(
                            payload_len = server_lsp.len(),
                            single_owner = view.is_single_owner,
                            "Processing incoming LSP message"
                        );

                        // Inspect LSP message structure
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&server_lsp) {
                            let method = val.get("method").and_then(|m| m.as_str());
                            let id = val.get("id").cloned();

                            // 1. Intercept "initialize": reply immediately with cached server capabilities
                            if method == Some("initialize") {
                                let req_id = id.unwrap_or(serde_json::json!(1));
                                let caps = if let Some(ref go) = view.workspace.go_engine {
                                    go.capabilities.read().await.clone()
                                } else if let Some(ref generic_eng) = view.workspace.generic_engine {
                                    generic_eng.capabilities.read().await.clone()
                                } else if let Some(ref backend) = view.workspace.backend {
                                    backend.capabilities.read().await.clone()
                                } else {
                                    None
                                };
                                let init_resp = serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": req_id,
                                    "result": {
                                        "capabilities": caps.unwrap_or_else(|| serde_json::json!({
                                            "textDocumentSync": 1,
                                            "hoverProvider": true,
                                            "definitionProvider": true,
                                            "referencesProvider": true,
                                            "documentSymbolProvider": true,
                                            "workspaceSymbolProvider": true
                                        })),
                                        "serverInfo": {
                                            "name": "prod-code-rci",
                                            "version": "0.1.0"
                                        }
                                    }
                                });
                                let client_resp = translator.translate_lsp_to_client(&init_resp.to_string());
                                let _ = out_tx.send(WireMessage::LspPayload(client_resp)).await;
                                continue;
                            }

                            // 2. Intercept "initialized": backend already initialized, consume without forwarding
                            if method == Some("initialized") {
                                continue;
                            }

                            // 3. Intercept "shutdown": reply cleanly
                            if method == Some("shutdown") {
                                let req_id = id.unwrap_or(serde_json::json!(1));
                                let shutdown_resp = serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": req_id,
                                    "result": null
                                });
                                let _ = out_tx.send(WireMessage::LspPayload(shutdown_resp.to_string())).await;
                                continue;
                            }

                            // 4. In-Memory RustEngine multi-core fast path: hover, definition, references, documentSymbol
                            if let Some(ref engine_lock) = view.workspace.rust_engine {
                                match method {
                                    Some("textDocument/hover") => {
                                        if let Some(params) = val.get("params") {
                                            let uri = params.get("textDocument").and_then(|td| td.get("uri")).and_then(|u| u.as_str()).unwrap_or("");
                                            let line = params.get("position").and_then(|p| p.get("line")).and_then(|l| l.as_u64()).unwrap_or(0) as u32;
                                            let col = params.get("position").and_then(|p| p.get("character")).and_then(|c| c.as_u64()).unwrap_or(0) as u32;
                                            let file_path = PathBuf::from(uri.trim_start_matches("file://"));

                                            let req_num = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
                                            let in_flight = ACTIVE_QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
                                            TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
                                            let query_start = Instant::now();

                                            tracing::info!(
                                                req = req_num,
                                                session = view.session_id,
                                                method = "textDocument/hover",
                                                file = %file_path.display(),
                                                pos = format!("{}:{}", line + 1, col + 1),
                                                in_flight,
                                                "🚀 [LSP START]"
                                            );

                                            // Acquire cheap snapshot (<1 µs) without holding mutex during query
                                            let engine_arc = Arc::clone(engine_lock);

                                            let req_id = id.clone().unwrap_or(serde_json::json!(1));
                                            let fp_clone = file_path.clone();
                                            let out_tx_task = out_tx.clone();
                                            let translator_task = translator.clone();
                                            let session_id = view.session_id;

                                            tokio::task::spawn(async move {
                                                let hover_res = {
                                                    // Hold the engine for the whole query: activating the session view and
                                                    // running the query under one lock keeps other sessions' buffers out and
                                                    // prevents a concurrent edit from cancelling this snapshot.
                                                    let mut engine = engine_arc.lock_owned().await;
                                                    tokio::task::spawn_blocking(move || {
                                                        if let Err(e) = engine.activate_session(session_id) {
                                                            tracing::warn!(error = %e, session = session_id, "session view activation failed");
                                                        }
                                                        engine.hover(&fp_clone, line + 1, col + 1).unwrap_or_else(|e| {
                                                            tracing::warn!(error = %e, session = session_id, "query failed");
                                                            None
                                                        })
                                                    })
                                                    .await
                                                    .unwrap_or(None)
                                                };

                                                let duration = query_start.elapsed();
                                                let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;
                                                let ms = duration.as_secs_f64() * 1000.0;
                                                let found = hover_res.is_some();

                                                if ms > 200.0 {
                                                    SLOW_QUERIES.fetch_add(1, Ordering::Relaxed);
                                                    tracing::warn!(
                                                        req = req_num,
                                                        session = session_id,
                                                        method = "textDocument/hover",
                                                        duration_ms = format!("{:.2}ms", ms),
                                                        found,
                                                        in_flight = remaining,
                                                        "⚠️ [LSP SLOW >200ms]"
                                                    );
                                                } else {
                                                    tracing::info!(
                                                        req = req_num,
                                                        session = session_id,
                                                        method = "textDocument/hover",
                                                        duration_ms = format!("{:.2}ms", ms),
                                                        found,
                                                        in_flight = remaining,
                                                        "✅ [LSP DONE]"
                                                    );
                                                }

                                                let resp = match hover_res {
                                                    Some(markup) => serde_json::json!({
                                                        "jsonrpc": "2.0",
                                                        "id": req_id,
                                                        "result": {
                                                            "contents": {
                                                                "kind": "markdown",
                                                                "value": markup
                                                            }
                                                        }
                                                    }),
                                                    None => serde_json::json!({
                                                        "jsonrpc": "2.0",
                                                        "id": req_id,
                                                        "result": null
                                                    }),
                                                };
                                                let client_resp = translator_task.translate_lsp_to_client(&resp.to_string());
                                                let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
                                            });
                                            continue;
                                        }
                                    }
                                    Some("textDocument/definition") => {
                                        if let Some(params) = val.get("params") {
                                            let uri = params.get("textDocument").and_then(|td| td.get("uri")).and_then(|u| u.as_str()).unwrap_or("");
                                            let line = params.get("position").and_then(|p| p.get("line")).and_then(|l| l.as_u64()).unwrap_or(0) as u32;
                                            let col = params.get("position").and_then(|p| p.get("character")).and_then(|c| c.as_u64()).unwrap_or(0) as u32;
                                            let file_path = PathBuf::from(uri.trim_start_matches("file://"));

                                            let req_num = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
                                            let in_flight = ACTIVE_QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
                                            TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
                                            let query_start = Instant::now();

                                            tracing::info!(
                                                req = req_num,
                                                session = view.session_id,
                                                method = "textDocument/definition",
                                                file = %file_path.display(),
                                                pos = format!("{}:{}", line + 1, col + 1),
                                                in_flight,
                                                "🚀 [LSP START]"
                                            );

                                            let engine_arc = Arc::clone(engine_lock);

                                            let req_id = id.clone().unwrap_or(serde_json::json!(1));
                                            let fp_clone = file_path.clone();
                                            let out_tx_task = out_tx.clone();
                                            let translator_task = translator.clone();
                                            let session_id = view.session_id;

                                            tokio::task::spawn(async move {
                                                let defs = {
                                                    // Hold the engine for the whole query: activating the session view and
                                                    // running the query under one lock keeps other sessions' buffers out and
                                                    // prevents a concurrent edit from cancelling this snapshot.
                                                    let mut engine = engine_arc.lock_owned().await;
                                                    tokio::task::spawn_blocking(move || {
                                                        if let Err(e) = engine.activate_session(session_id) {
                                                            tracing::warn!(error = %e, session = session_id, "session view activation failed");
                                                        }
                                                        engine.goto_definition(&fp_clone, line + 1, col + 1).unwrap_or_else(|e| {
                                                            tracing::warn!(error = %e, session = session_id, "query failed");
                                                            Vec::new()
                                                        })
                                                    })
                                                    .await
                                                    .unwrap_or_default()
                                                };

                                                let duration = query_start.elapsed();
                                                let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;
                                                let ms = duration.as_secs_f64() * 1000.0;
                                                let count = defs.len();

                                                if ms > 200.0 {
                                                    SLOW_QUERIES.fetch_add(1, Ordering::Relaxed);
                                                    tracing::warn!(
                                                        req = req_num,
                                                        session = session_id,
                                                        method = "textDocument/definition",
                                                        duration_ms = format!("{:.2}ms", ms),
                                                        targets = count,
                                                        in_flight = remaining,
                                                        "⚠️ [LSP SLOW >200ms]"
                                                    );
                                                } else {
                                                    tracing::info!(
                                                        req = req_num,
                                                        session = session_id,
                                                        method = "textDocument/definition",
                                                        duration_ms = format!("{:.2}ms", ms),
                                                        targets = count,
                                                        in_flight = remaining,
                                                        "✅ [LSP DONE]"
                                                    );
                                                }

                                                let locations: Vec<_> = defs.into_iter().map(|t| {
                                                    serde_json::json!({
                                                        "uri": format!("file://{}", t.path.display()),
                                                        "range": {
                                                            "start": { "line": t.line.saturating_sub(1), "character": t.col.saturating_sub(1) },
                                                            "end": { "line": t.line.saturating_sub(1), "character": t.col.saturating_sub(1) }
                                                        }
                                                    })
                                                }).collect();

                                                let resp = serde_json::json!({
                                                    "jsonrpc": "2.0",
                                                    "id": req_id,
                                                    "result": locations
                                                });
                                                let client_resp = translator_task.translate_lsp_to_client(&resp.to_string());
                                                let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
                                            });
                                            continue;
                                        }
                                    }
                                    Some("textDocument/references") => {
                                        if let Some(params) = val.get("params") {
                                            let uri = params.get("textDocument").and_then(|td| td.get("uri")).and_then(|u| u.as_str()).unwrap_or("");
                                            let line = params.get("position").and_then(|p| p.get("line")).and_then(|l| l.as_u64()).unwrap_or(0) as u32;
                                            let col = params.get("position").and_then(|p| p.get("character")).and_then(|c| c.as_u64()).unwrap_or(0) as u32;
                                            let file_path = PathBuf::from(uri.trim_start_matches("file://"));

                                            let req_num = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
                                            let in_flight = ACTIVE_QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
                                            TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
                                            let query_start = Instant::now();

                                            tracing::info!(
                                                req = req_num,
                                                session = view.session_id,
                                                method = "textDocument/references",
                                                file = %file_path.display(),
                                                pos = format!("{}:{}", line + 1, col + 1),
                                                in_flight,
                                                "🚀 [LSP START]"
                                            );

                                            let engine_arc = Arc::clone(engine_lock);

                                            let req_id = id.clone().unwrap_or(serde_json::json!(1));
                                            let fp_clone = file_path.clone();
                                            let out_tx_task = out_tx.clone();
                                            let translator_task = translator.clone();
                                            let session_id = view.session_id;

                                            tokio::task::spawn(async move {
                                                let refs = {
                                                    // Hold the engine for the whole query: activating the session view and
                                                    // running the query under one lock keeps other sessions' buffers out and
                                                    // prevents a concurrent edit from cancelling this snapshot.
                                                    let mut engine = engine_arc.lock_owned().await;
                                                    tokio::task::spawn_blocking(move || {
                                                        if let Err(e) = engine.activate_session(session_id) {
                                                            tracing::warn!(error = %e, session = session_id, "session view activation failed");
                                                        }
                                                        engine.find_all_refs(&fp_clone, line + 1, col + 1).unwrap_or_else(|e| {
                                                            tracing::warn!(error = %e, session = session_id, "query failed");
                                                            Vec::new()
                                                        })
                                                    })
                                                    .await
                                                    .unwrap_or_default()
                                                };

                                                let duration = query_start.elapsed();
                                                let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;
                                                let ms = duration.as_secs_f64() * 1000.0;
                                                let count = refs.len();

                                                if ms > 200.0 {
                                                    SLOW_QUERIES.fetch_add(1, Ordering::Relaxed);
                                                    tracing::warn!(
                                                        req = req_num,
                                                        session = session_id,
                                                        method = "textDocument/references",
                                                        duration_ms = format!("{:.2}ms", ms),
                                                        references = count,
                                                        in_flight = remaining,
                                                        "⚠️ [LSP SLOW >200ms]"
                                                    );
                                                } else {
                                                    tracing::info!(
                                                        req = req_num,
                                                        session = session_id,
                                                        method = "textDocument/references",
                                                        duration_ms = format!("{:.2}ms", ms),
                                                        references = count,
                                                        in_flight = remaining,
                                                        "✅ [LSP DONE]"
                                                    );
                                                }

                                                let locations: Vec<_> = refs.into_iter().map(|t| {
                                                    serde_json::json!({
                                                        "uri": format!("file://{}", t.path.display()),
                                                        "range": {
                                                            "start": { "line": t.line.saturating_sub(1), "character": t.col.saturating_sub(1) },
                                                            "end": { "line": t.line.saturating_sub(1), "character": t.col.saturating_sub(1) }
                                                        }
                                                    })
                                                }).collect();

                                                let resp = serde_json::json!({
                                                    "jsonrpc": "2.0",
                                                    "id": req_id,
                                                    "result": locations
                                                });
                                                let client_resp = translator_task.translate_lsp_to_client(&resp.to_string());
                                                let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
                                            });
                                            continue;
                                        }
                                    }
                                    Some("textDocument/documentSymbol") => {
                                        if let Some(params) = val.get("params") {
                                             let uri = params.get("textDocument").and_then(|td| td.get("uri")).and_then(|u| u.as_str()).unwrap_or("").to_string();
                                             let file_path = PathBuf::from(uri.trim_start_matches("file://"));

                                             let req_num = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
                                             let in_flight = ACTIVE_QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
                                             TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
                                             let query_start = Instant::now();

                                             tracing::info!(
                                                 req = req_num,
                                                 session = view.session_id,
                                                 method = "textDocument/documentSymbol",
                                                 file = %file_path.display(),
                                                 in_flight,
                                                 "🚀 [LSP START]"
                                             );

                                             let engine_arc = Arc::clone(engine_lock);

                                             let req_id = id.clone().unwrap_or(serde_json::json!(1));
                                             let fp_clone = file_path.clone();
                                             let out_tx_task = out_tx.clone();
                                             let translator_task = translator.clone();
                                             let session_id = view.session_id;

                                             tokio::task::spawn(async move {
                                                 let syms = {
                                                     // Hold the engine for the whole query: activating the session view and
                                                     // running the query under one lock keeps other sessions' buffers out and
                                                     // prevents a concurrent edit from cancelling this snapshot.
                                                     let mut engine = engine_arc.lock_owned().await;
                                                     tokio::task::spawn_blocking(move || {
                                                         if let Err(e) = engine.activate_session(session_id) {
                                                             tracing::warn!(error = %e, session = session_id, "session view activation failed");
                                                         }
                                                         engine.document_symbols(&fp_clone).unwrap_or_else(|e| {
                                                             tracing::warn!(error = %e, session = session_id, "query failed");
                                                             Vec::new()
                                                         })
                                                     })
                                                     .await
                                                     .unwrap_or_default()
                                                 };

                                                 let duration = query_start.elapsed();
                                                 let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;
                                                 let ms = duration.as_secs_f64() * 1000.0;
                                                 let count = syms.len();

                                                 if ms > 200.0 {
                                                     SLOW_QUERIES.fetch_add(1, Ordering::Relaxed);
                                                     tracing::warn!(
                                                         req = req_num,
                                                         session = session_id,
                                                         method = "textDocument/documentSymbol",
                                                         duration_ms = format!("{:.2}ms", ms),
                                                         symbols = count,
                                                         in_flight = remaining,
                                                         "⚠️ [LSP SLOW >200ms]"
                                                     );
                                                 } else {
                                                     tracing::info!(
                                                         req = req_num,
                                                         session = session_id,
                                                         method = "textDocument/documentSymbol",
                                                         duration_ms = format!("{:.2}ms", ms),
                                                         symbols = count,
                                                         in_flight = remaining,
                                                         "✅ [LSP DONE]"
                                                     );
                                                 }

                                                 let sym_list: Vec<_> = syms.into_iter().map(|s| {
                                                     let kind_num = match s.kind.as_str() {
                                                         "Fn" | "Function" => 12,
                                                         "Struct" => 23,
                                                         "Enum" => 10,
                                                         "Const" | "Constant" => 14,
                                                         "Trait" => 11,
                                                         "Module" => 2,
                                                         _ => 13,
                                                     };
                                                     serde_json::json!({
                                                         "name": s.name,
                                                         "kind": kind_num,
                                                         "location": {
                                                             "uri": uri,
                                                             "range": {
                                                                 "start": { "line": s.line.saturating_sub(1), "character": 0 },
                                                                 "end": { "line": s.line.saturating_sub(1), "character": 0 }
                                                             }
                                                         },
                                                         "containerName": s.detail
                                                     })
                                                 }).collect();

                                                 let resp = serde_json::json!({
                                                     "jsonrpc": "2.0",
                                                     "id": req_id,
                                                     "result": sym_list
                                                 });
                                                 let client_resp = translator_task.translate_lsp_to_client(&resp.to_string());
                                                 let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
                                             });
                                             continue;
                                         }
                                    }
                                    Some("textDocument/didOpen") => {
                                        if let Some(params) = val.get("params") {
                                            let uri = params.get("textDocument").and_then(|td| td.get("uri")).and_then(|u| u.as_str()).unwrap_or("");
                                            let file_path = PathBuf::from(uri.trim_start_matches("file://"));
                                            if let Some(text) = params.get("textDocument").and_then(|td| td.get("text")).and_then(|t| t.as_str()) {
                                                let edit_start = Instant::now();
                                                let text_len = text.len();
                                                {
                                                    let mut engine = engine_lock.lock().await;
                                                    if let Err(e) = engine.set_session_overlay(view.session_id, &file_path, Some(text.to_string())) {
                                                        tracing::warn!(error = %e, file = %file_path.display(), "session overlay update failed");
                                                    }
                                                }
                                                let ms = edit_start.elapsed().as_secs_f64() * 1000.0;
                                                tracing::info!(
                                                    session = view.session_id,
                                                    file = %file_path.display(),
                                                    bytes = text_len,
                                                    duration_ms = format!("{:.2}ms", ms),
                                                    "📝 [OVERLAY] didOpen recorded as session buffer in Salsa DB"
                                                );
                                            }
                                        }
                                    }
                                    Some("textDocument/didChange") => {
                                        if let Some(params) = val.get("params") {
                                            let uri = params.get("textDocument").and_then(|td| td.get("uri")).and_then(|u| u.as_str()).unwrap_or("");
                                            let file_path = PathBuf::from(uri.trim_start_matches("file://"));
                                            let first = params
                                                .get("contentChanges")
                                                .and_then(|c| c.as_array())
                                                .and_then(|arr| arr.first())
                                                .and_then(|c| c.get("text"))
                                                .and_then(|t| t.as_str());
                                            if let Some(text) = first {
                                                let edit_start = Instant::now();
                                                let text_len = text.len();
                                                {
                                                    let mut engine = engine_lock.lock().await;
                                                    if let Err(e) = engine.set_session_overlay(view.session_id, &file_path, Some(text.to_string())) {
                                                        tracing::warn!(error = %e, file = %file_path.display(), "session overlay update failed");
                                                    }
                                                }
                                                let ms = edit_start.elapsed().as_secs_f64() * 1000.0;
                                                tracing::info!(
                                                    session = view.session_id,
                                                    file = %file_path.display(),
                                                    bytes = text_len,
                                                    duration_ms = format!("{:.2}ms", ms),
                                                    "📝 [OVERLAY] didChange recorded as session buffer in Salsa DB"
                                                );
                                            }
                                        }
                                    }
                                    Some("textDocument/didClose") => {
                                        if let Some(params) = val.get("params") {
                                            let uri = params.get("textDocument").and_then(|td| td.get("uri")).and_then(|u| u.as_str()).unwrap_or("");
                                            let file_path = PathBuf::from(uri.trim_start_matches("file://"));
                                            let mut engine = engine_lock.lock().await;
                                            if let Err(e) = engine.clear_session_overlay(view.session_id, &file_path) {
                                                tracing::warn!(error = %e, file = %file_path.display(), "session overlay close failed");
                                            }
                                        }
                                        continue;
                                    }
                                    _ => {}
                                }
                            }

                            // 5. Fallback handling for textDocument/didOpen vs didChange on backend worker
                            if let (Some("textDocument/didOpen"), Some(backend)) = (method, &view.workspace.backend) {
                                let uri = val.get("params")
                                    .and_then(|p| p.get("textDocument"))
                                    .and_then(|td| td.get("uri"))
                                    .and_then(|u| u.as_str())
                                    .unwrap_or("");
                                let is_open = backend.open_files.read().await.contains(uri);
                                if is_open {
                                    let text = val.get("params")
                                        .and_then(|p| p.get("textDocument"))
                                        .and_then(|td| td.get("text"))
                                        .and_then(|t| t.as_str())
                                        .unwrap_or("");
                                    let version = val.get("params")
                                        .and_then(|p| p.get("textDocument"))
                                        .and_then(|td| td.get("version"))
                                        .and_then(|v| v.as_i64())
                                        .unwrap_or(2);
                                    let did_change = serde_json::json!({
                                        "jsonrpc": "2.0",
                                        "method": "textDocument/didChange",
                                        "params": {
                                            "textDocument": {
                                                "uri": uri,
                                                "version": version
                                            },
                                            "contentChanges": [
                                                { "text": text }
                                            ]
                                        }
                                    });
                                    let _ = backend.send_lsp(&did_change.to_string()).await;
                                    continue;
                                } else {
                                    backend.open_files.write().await.insert(uri.to_string());
                                }
                            }

                            // 5b. Supervised GoEngine fast path
                            if let Some(ref go) = view.workspace.go_engine {
                                match method {
                                    Some("textDocument/didOpen") => {
                                        let uri = val.get("params").and_then(|p| p.get("textDocument")).and_then(|td| td.get("uri")).and_then(|u| u.as_str()).unwrap_or("");
                                        let text = val.get("params").and_then(|p| p.get("textDocument")).and_then(|td| td.get("text")).and_then(|t| t.as_str()).unwrap_or("");
                                        let _ = go.did_open(uri, text).await;
                                        continue;
                                    }
                                    Some("textDocument/didChange") => {
                                        let uri = val.get("params").and_then(|p| p.get("textDocument")).and_then(|td| td.get("uri")).and_then(|u| u.as_str()).unwrap_or("");
                                        let version = val.get("params").and_then(|p| p.get("textDocument")).and_then(|td| td.get("version")).and_then(|v| v.as_i64()).unwrap_or(1) as i32;
                                        let text = val.get("params").and_then(|p| p.get("contentChanges")).and_then(|c| c.as_array()).and_then(|a| a.first()).and_then(|ch| ch.get("text")).and_then(|t| t.as_str()).unwrap_or("");
                                        let _ = go.did_change(uri, text, version).await;
                                        continue;
                                    }
                                    Some("textDocument/didClose") => {
                                        let uri = val.get("params").and_then(|p| p.get("textDocument")).and_then(|td| td.get("uri")).and_then(|u| u.as_str()).unwrap_or("");
                                        let _ = go.did_close(uri).await;
                                        continue;
                                    }
                                    Some(m) if id.is_some() => {
                                        let req_id_log = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
                                        let in_flight = ACTIVE_QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
                                        TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
                                        let start = Instant::now();

                                        tracing::info!(
                                            req = req_id_log,
                                            session = view.session_id,
                                            method = m,
                                            in_flight,
                                            "🚀 [LSP START] dispatching to GoEngine"
                                        );

                                        let params = val.get("params").cloned().unwrap_or(serde_json::json!({}));
                                        let out_tx_task = out_tx.clone();
                                        let go_clone = Arc::clone(go);
                                        let translator_task = translator.clone();
                                        let session_id = view.session_id;
                                        let method_str = m.to_string();
                                        let req_id = id.clone();

                                        tokio::task::spawn(async move {
                                            let resp_res = go_clone.send_request(&method_str, params).await;
                                            let duration = start.elapsed();
                                            let duration_ms = duration.as_secs_f64() * 1000.0;
                                            let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;

                                            if duration_ms > 200.0 {
                                                SLOW_QUERIES.fetch_add(1, Ordering::Relaxed);
                                                tracing::warn!(
                                                    req = req_id_log,
                                                    session = session_id,
                                                    method = %method_str,
                                                    duration_ms = %format!("{:.2}ms", duration_ms),
                                                    in_flight = remaining,
                                                    "⚠️ [LSP SLOW >200ms] GoEngine query exceeded threshold"
                                                );
                                            } else {
                                                tracing::info!(
                                                    req = req_id_log,
                                                    session = session_id,
                                                    method = %method_str,
                                                    duration_ms = %format!("{:.2}ms", duration_ms),
                                                    in_flight = remaining,
                                                    "✅ [LSP DONE] GoEngine query complete"
                                                );
                                            }

                                            match resp_res {
                                                Ok(mut resp) => {
                                                    if let Some(ref r_id) = req_id {
                                                        resp["id"] = r_id.clone();
                                                    }
                                                    let client_resp = translator_task.translate_lsp_to_client(&resp.to_string());
                                                    let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
                                                }
                                                Err(err) => {
                                                    let err_resp = serde_json::json!({
                                                        "jsonrpc": "2.0",
                                                        "id": req_id,
                                                        "error": { "code": -32603, "message": err.to_string() }
                                                    });
                                                    let _ = out_tx_task.send(WireMessage::LspPayload(err_resp.to_string())).await;
                                                }
                                            }
                                        });
                                        continue;
                                    }
                                    Some(m) => {
                                        let params = val.get("params").cloned().unwrap_or(serde_json::json!({}));
                                        let _ = go.send_notification(m, params).await;
                                        continue;
                                    }
                                    None => {}
                                }
                            }

                            // 5c. Supervised GenericLspEngine fast path
                            if let Some(ref generic_eng) = view.workspace.generic_engine {
                                if let (Some(m), Some(req_id)) = (method, &id) {
                                    let req_id_log = NEXT_REQ_ID.fetch_add(1, Ordering::Relaxed);
                                    let in_flight = ACTIVE_QUERIES.fetch_add(1, Ordering::Relaxed) + 1;
                                    TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
                                    let start = Instant::now();

                                    tracing::info!(
                                        req = req_id_log,
                                        session = view.session_id,
                                        method = m,
                                        in_flight,
                                        "🚀 [LSP START] dispatching to GenericLspEngine"
                                    );

                                    let params = val.get("params").cloned().unwrap_or(serde_json::json!({}));
                                    let out_tx_task = out_tx.clone();
                                    let generic_eng_clone = Arc::clone(generic_eng);
                                    let translator_task = translator.clone();
                                    let session_id = view.session_id;
                                    let method_str = m.to_string();
                                    let r_id = req_id.clone();

                                    tokio::task::spawn(async move {
                                        let resp_res = generic_eng_clone.send_request(&method_str, params).await;
                                        let duration = start.elapsed();
                                        let duration_ms = duration.as_secs_f64() * 1000.0;
                                        let remaining = ACTIVE_QUERIES.fetch_sub(1, Ordering::Relaxed) - 1;

                                        if duration_ms > 200.0 {
                                            SLOW_QUERIES.fetch_add(1, Ordering::Relaxed);
                                            tracing::warn!(
                                                req = req_id_log,
                                                session = session_id,
                                                method = %method_str,
                                                duration_ms = %format!("{:.2}ms", duration_ms),
                                                in_flight = remaining,
                                                "⚠️ [LSP SLOW >200ms] GenericLspEngine query exceeded threshold"
                                            );
                                        } else {
                                            tracing::info!(
                                                req = req_id_log,
                                                session = session_id,
                                                method = %method_str,
                                                duration_ms = %format!("{:.2}ms", duration_ms),
                                                in_flight = remaining,
                                                "✅ [LSP DONE] GenericLspEngine query complete"
                                            );
                                        }

                                        match resp_res {
                                            Ok(mut resp) => {
                                                resp["id"] = r_id;
                                                let client_resp = translator_task.translate_lsp_to_client(&resp.to_string());
                                                let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
                                            }
                                            Err(err) => {
                                                let err_resp = serde_json::json!({
                                                    "jsonrpc": "2.0",
                                                    "id": r_id,
                                                    "error": { "code": -32603, "message": err.to_string() }
                                                });
                                                let _ = out_tx_task.send(WireMessage::LspPayload(err_resp.to_string())).await;
                                            }
                                        }
                                    });
                                    continue;
                                } else if let Some(m) = method {
                                    let params = val.get("params").cloned().unwrap_or(serde_json::json!({}));
                                    let _ = generic_eng.send_notification(m, params).await;
                                    continue;
                                }
                            }

                            // 6. Handle "textDocument/didClose"
                            if let (Some("textDocument/didClose"), Some(backend)) = (method, &view.workspace.backend) {
                                let uri = val.get("params")
                                    .and_then(|p| p.get("textDocument"))
                                    .and_then(|td| td.get("uri"))
                                    .and_then(|u| u.as_str())
                                    .unwrap_or("");
                                backend.open_files.write().await.remove(uri);
                            }

                            // 7. If no backend is attached and client expects a response, return empty result
                            if let (Some(req_id), None, None, None, None) = (
                                id,
                                &view.workspace.backend,
                                &view.workspace.rust_engine,
                                &view.workspace.go_engine,
                                &view.workspace.generic_engine,
                            ) {
                                let empty_resp = serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": req_id,
                                    "result": null
                                });
                                let _ = out_tx.send(WireMessage::LspPayload(empty_resp.to_string())).await;
                                continue;
                            }
                        }

                        if let Some(backend) = &view.workspace.backend {
                            let _ = backend.send_lsp(&server_lsp).await.inspect_err(|e| {
                                tracing::error!(error = %e, "Failed to forward LSP to backend worker");
                            });
                        }
                    }
                    Some(Ok(WireMessage::SyncRequest(req))) => {
                        let start = Instant::now();
                        let mut files_updated = 0;
                        let mut files_deleted = 0;
                        let mut bytes_transferred = 0;

                        for delta in &req.files {
                            let target_path = view.workspace.root.join(&delta.relative_path);
                            match &delta.content {
                                Some(content_bytes) => {
                                    if let Some(parent) = target_path.parent() {
                                        let _ = tokio::fs::create_dir_all(parent).await;
                                    }
                                    bytes_transferred += content_bytes.len();
                                    if tokio::fs::write(&target_path, content_bytes).await.is_ok() {
                                        files_updated += 1;
                                    }
                                    // The workspace is this worktree's own: synced files are its
                                    // new base, visible to every session except one that still
                                    // holds an unsaved buffer for the same path.
                                    if let (Some(engine_lock), Ok(text)) =
                                        (&view.workspace.rust_engine, std::str::from_utf8(content_bytes))
                                    {
                                        let mut engine = engine_lock.lock().await;
                                        if let Err(e) = engine.update_base(&target_path, Some(text.to_string())) {
                                            tracing::warn!(error = %e, file = %target_path.display(), "base update failed");
                                        }
                                    }
                                }
                                None => {
                                    if target_path.exists()
                                        && tokio::fs::remove_file(&target_path).await.is_ok()
                                    {
                                        files_deleted += 1;
                                    }
                                    if let Some(engine_lock) = &view.workspace.rust_engine {
                                        let mut engine = engine_lock.lock().await;
                                        if let Err(e) = engine.update_base(&target_path, None) {
                                            tracing::warn!(error = %e, file = %target_path.display(), "base removal failed");
                                        }
                                    }
                                }
                            }
                        }

                        if req.clean_others
                            && let Some(engine_lock) = &view.workspace.rust_engine
                        {
                            // The request is the session's complete dirty set: any other
                            // overlay this session still holds is stale (reverted or committed).
                            let keep: Vec<PathBuf> = req
                                .files
                                .iter()
                                .map(|delta| view.workspace.root.join(&delta.relative_path))
                                .collect();
                            let mut engine = engine_lock.lock().await;
                            match engine.retain_session_overlays(view.session_id, &keep) {
                                Ok(dropped) if dropped > 0 => tracing::info!(
                                    session = view.session_id,
                                    dropped,
                                    "🧹 [OVERLAY] dropped stale session buffers after full dirty sync"
                                ),
                                Ok(_) => {}
                                Err(e) => tracing::warn!(error = %e, session = view.session_id, "failed to drop stale session buffers"),
                            }
                        }

                        let duration_ms = start.elapsed().as_millis() as u64;
                        let _ = out_tx
                            .send(WireMessage::SyncResponse(SyncResponse {
                                files_updated,
                                files_deleted,
                                bytes_transferred,
                                duration_ms,
                                server_workspace_root: view.workspace.root.to_string_lossy().to_string(),
                            }))
                            .await;
                    }
                    Some(Ok(WireMessage::Disconnect { reason })) => {
                        tracing::info!(reason, "Client terminated session");
                        break;
                    }
                    Some(Ok(WireMessage::StatusRequest)) => {
                        let _ = out_tx
                            .send(WireMessage::StatusResponse(StatusResponse {
                                server_pid: std::process::id(),
                                uptime_seconds: 0,
                                active_sessions: 1,
                                loaded_workspaces: 1,
                                detected_engines: vec![view.workspace.engine.clone()],
                                memory_rss_bytes: memory::get_process_rss_bytes(),
                                total_queries: TOTAL_QUERIES.load(Ordering::Relaxed),
                                active_queries: ACTIVE_QUERIES.load(Ordering::Relaxed),
                            }))
                            .await;
                    }
                    Some(Err(e)) => {
                        tracing::error!(error = %e, "TCP frame decode error");
                        break;
                    }
                    None => {
                        tracing::info!("Client disconnected");
                        break;
                    }
                    _ => {}
                }
            }

            backend_msg = async {
                if let Some(ref mut rx) = backend_rx {
                    rx.recv().await
                } else {
                    futures_util::future::pending::<Result<String, tokio::sync::broadcast::error::RecvError>>().await
                }
            } => {
                match backend_msg {
                    Ok(server_lsp) => {
                        let client_lsp = translator.translate_lsp_to_client(&server_lsp);
                        if out_tx.send(WireMessage::LspPayload(client_lsp)).await.is_err() {
                            tracing::error!("Failed to send LSP message to client channel");
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(skipped, "Session backend receiver lagged");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        tracing::warn!("Backend worker broadcast closed");
                        break;
                    }
                }
            }
        }
    }
    drop(out_tx);
    let _ = writer_handle.await;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                "info,prod_code_gateway=debug,prod_code_engine_rust=debug".into()
            }),
        )
        .init();
    let cli = ServerCli::parse();
    prefer_rustup_toolchain();

    tracing::info!(
        "prod-code gateway daemon starting on {} (storage: {:?})",
        cli.bind,
        cli.storage
    );

    let state = Arc::new(ServerState::new(cli.storage));
    let listener = TcpListener::bind(cli.bind).await?;
    tracing::info!("prod-code gateway listening on {}", cli.bind);

    tokio::spawn(janitor(
        Arc::clone(&state),
        cli.idle_evict_secs,
        cli.prune_worktree_days,
    ));

    loop {
        let (socket, addr) = listener.accept().await?;
        let state_clone = Arc::clone(&state);
        tokio::spawn(async move {
            if let Err(err) = handle_client(socket, addr, state_clone).await {
                tracing::error!(%addr, %err, "Error in client connection");
            }
        });
    }
}

/// Puts `~/.cargo/bin` first on PATH so remote commands and rust-analyzer's `cargo metadata`
/// use the rustup toolchain the workspaces were built with, not a distro/snap cargo that a
/// systemd user session may resolve first.
fn prefer_rustup_toolchain() {
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let cargo_bin = PathBuf::from(home).join(".cargo/bin");
    if !cargo_bin.is_dir() {
        return;
    }
    let current = std::env::var_os("PATH").unwrap_or_default();
    let mut paths: Vec<PathBuf> = std::env::split_paths(&current).collect();
    paths.retain(|p| p != &cargo_bin);
    paths.insert(0, cargo_bin.clone());
    if let Ok(joined) = std::env::join_paths(paths) {
        // SAFETY: called once at startup before any other thread exists.
        unsafe { std::env::set_var("PATH", joined) };
        tracing::info!(cargo_bin = %cargo_bin.display(), "rustup toolchain put first on PATH");
    }
}

/// Periodically unloads idle engines and prunes stale worktree workspace directories.
async fn janitor(state: Arc<ServerState>, idle_evict_secs: u64, prune_worktree_days: u64) {
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(60));
    ticker.tick().await;
    loop {
        ticker.tick().await;
        if idle_evict_secs > 0 {
            let evicted = state
                .workspace_manager
                .evict_idle(std::time::Duration::from_secs(idle_evict_secs))
                .await;
            for root in evicted {
                tracing::info!(workspace = %root.display(), idle_secs = idle_evict_secs, "💤 [EVICT] unloaded idle workspace engine");
            }
        }
        if prune_worktree_days > 0 {
            workspace::prune_stale_worktree_dirs(
                &state.storage_root,
                std::time::Duration::from_secs(prune_worktree_days * 86_400),
                &state.workspace_manager,
            )
            .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_changed_since_reports_new_changed_and_deleted() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::write(root.join("src/a.rs"), "a").unwrap();
        std::fs::write(root.join("src/gone.rs"), "g").unwrap();
        std::fs::write(root.join("Cargo.lock"), "l1").unwrap();
        let before = snapshot_tree(root);

        std::fs::write(root.join("src/a.rs"), "a formatted").unwrap();
        std::fs::write(root.join("src/new.rs"), "n").unwrap();
        std::fs::remove_file(root.join("src/gone.rs")).unwrap();
        std::fs::write(root.join("target/debug/junk.o"), "x").unwrap();

        let changed = changed_since(root, &before);
        let names: Vec<(&str, bool)> = changed
            .iter()
            .map(|f| (f.relative_path.as_str(), f.content.is_some()))
            .collect();
        assert_eq!(
            names,
            vec![
                ("src/a.rs", true),
                ("src/gone.rs", false),
                ("src/new.rs", true)
            ]
        );
        assert_eq!(
            changed[0].content.as_deref(),
            Some(b"a formatted".as_slice())
        );
    }

    #[tokio::test]
    async fn test_sync_probe_seeds_and_reconciles() {
        let storage = tempfile::tempdir().unwrap();
        let origin = storage.path().join("repo");
        std::fs::create_dir_all(origin.join("src")).unwrap();
        std::fs::write(origin.join("Cargo.toml"), "[package]\nname = \"repo\"\n").unwrap();
        std::fs::write(origin.join("src/lib.rs"), "pub fn a() {}").unwrap();
        std::fs::write(origin.join("src/only_in_origin.rs"), "pub fn gone() {}").unwrap();
        let manager = WorkspaceManager::new();

        let req = SyncProbeRequest {
            client_workspace_root: "/tmp/wt".to_string(),
            base_workspace_name: Some("repo--wt-0001".to_string()),
            seed_from: Some("repo".to_string()),
            files: vec![
                FileStamp {
                    relative_path: "Cargo.toml".to_string(),
                    size: 24,
                    hash: content_hash(b"[package]\nname = \"repo\"\n"),
                },
                FileStamp {
                    relative_path: "src/lib.rs".to_string(),
                    size: 21,
                    hash: content_hash(b"pub fn a() -> u8 {}"),
                },
                FileStamp {
                    relative_path: "src/new.rs".to_string(),
                    size: 3,
                    hash: content_hash(b"// n"),
                },
            ],
        };
        let resp = apply_sync_probe(storage.path(), &manager, req).await;
        assert!(resp.seeded);
        assert_eq!(resp.files_deleted, 1);
        assert_eq!(
            resp.missing,
            vec!["src/lib.rs".to_string(), "src/new.rs".to_string()]
        );
        let wt = storage.path().join("repo--wt-0001");
        assert!(wt.join("Cargo.toml").exists());
        assert!(!wt.join("src/only_in_origin.rs").exists());
        assert!(
            origin.join("src/only_in_origin.rs").exists(),
            "origin copy untouched"
        );

        // A second probe on the now-populated directory does not seed again.
        let again = apply_sync_probe(
            storage.path(),
            &manager,
            SyncProbeRequest {
                client_workspace_root: "/tmp/wt".to_string(),
                base_workspace_name: Some("repo--wt-0001".to_string()),
                seed_from: Some("repo".to_string()),
                files: vec![FileStamp {
                    relative_path: "Cargo.toml".to_string(),
                    size: 24,
                    hash: content_hash(b"[package]\nname = \"repo\"\n"),
                }],
            },
        )
        .await;
        assert!(!again.seeded);
        assert!(again.missing.is_empty());
        assert_eq!(
            again.files_deleted, 1,
            "src/lib.rs is not in the manifest any more"
        );
    }

    #[tokio::test]
    async fn test_server_state_status() {
        let temp = tempfile::tempdir().unwrap();
        let state = ServerState::new(temp.path().to_path_buf());
        let status = state.status().await;
        assert_eq!(status.server_pid, std::process::id());
        assert_eq!(status.active_sessions, 0);
        assert_eq!(status.loaded_workspaces, 0);
        assert!(
            status
                .detected_engines
                .contains(&"rust (ra_ap_ide)".to_string())
        );
    }

    #[test]
    fn test_engine_detection() {
        use prod_code_protocol::messages::EngineKind;

        let temp = tempfile::tempdir().unwrap();
        assert_eq!(detect_engine(temp.path()), EngineKind::Generic);

        std::fs::write(temp.path().join("Cargo.toml"), "").unwrap();
        assert_eq!(detect_engine(temp.path()), EngineKind::Rust);

        let go_temp = tempfile::tempdir().unwrap();
        std::fs::write(go_temp.path().join("go.mod"), "").unwrap();
        assert_eq!(detect_engine(go_temp.path()), EngineKind::Go);

        let py_temp = tempfile::tempdir().unwrap();
        std::fs::write(py_temp.path().join("pyproject.toml"), "").unwrap();
        assert_eq!(detect_engine(py_temp.path()), EngineKind::Python);

        let ts_temp = tempfile::tempdir().unwrap();
        std::fs::write(ts_temp.path().join("package.json"), "").unwrap();
        assert_eq!(detect_engine(ts_temp.path()), EngineKind::TypeScript);
    }

    #[tokio::test]
    async fn test_apply_sync_create_and_delete() {
        use prod_code_protocol::FileDelta;

        let storage_temp = tempfile::tempdir().unwrap();
        let client_root = "/Users/testuser/Projects/my-app";

        let req = SyncRequest {
            client_workspace_root: client_root.to_string(),
            files: vec![
                FileDelta {
                    relative_path: "src/lib.rs".to_string(),
                    content: Some(b"pub fn add(a: i32, b: i32) -> i32 { a + b }".to_vec()),
                    is_executable: false,
                },
                FileDelta {
                    relative_path: "README.md".to_string(),
                    content: Some(b"# My App".to_vec()),
                    is_executable: false,
                },
            ],
            clean_others: false,
            base_workspace_name: None,
        };

        let resp = apply_sync(storage_temp.path(), &WorkspaceManager::new(), req).await;
        assert_eq!(resp.files_updated, 2);
        assert_eq!(resp.files_deleted, 0);

        let app_dir = storage_temp.path().join("my-app");
        assert!(app_dir.join("src/lib.rs").exists());
        assert!(app_dir.join("README.md").exists());
        let content = std::fs::read_to_string(app_dir.join("src/lib.rs")).unwrap();
        assert!(content.contains("pub fn add"));

        // Now test deleting README.md
        let del_req = SyncRequest {
            client_workspace_root: client_root.to_string(),
            files: vec![FileDelta {
                relative_path: "README.md".to_string(),
                content: None,
                is_executable: false,
            }],
            clean_others: false,
            base_workspace_name: None,
        };

        let del_resp = apply_sync(storage_temp.path(), &WorkspaceManager::new(), del_req).await;
        assert_eq!(del_resp.files_updated, 0);
        assert_eq!(del_resp.files_deleted, 1);
        assert!(!app_dir.join("README.md").exists());
        assert!(app_dir.join("src/lib.rs").exists());
    }
}
