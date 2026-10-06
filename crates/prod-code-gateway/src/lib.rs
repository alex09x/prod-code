/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! prod-code gateway daemon: multi-tenant server for remote code intelligence over 10 GbE LAN.
//!
//! The daemon is a library with a thin binary on top, so that the pieces a socket normally
//! stands in front of — the workspace manager, the dispatch, the language server backends —
//! can be exercised directly by tests.

pub mod admission;
pub mod backend;
pub mod cpp_index;
pub mod detect;
pub mod editor_proxy;
pub mod embed;
pub mod exec_shim;
pub mod memory;
mod metrics;
pub mod priming;
pub mod python_cache;
pub mod search;
pub mod shadow;
pub mod swift_cache;
pub mod ts_cache;
pub mod workspace;
pub mod sync;
pub use sync::*;
pub(crate) mod lsp;
pub(crate) use lsp::*;
pub mod state;
pub use state::*;
pub mod seed_cache;
pub use seed_cache::*;
pub mod engines;
pub(crate) mod gossip;
pub(crate) mod janitor_task;
pub(crate) mod lsp_handlers;
pub mod server;

pub use engines::*;
pub(crate) use gossip::*;
pub(crate) use janitor_task::*;
pub(crate) use lsp_handlers::*;
pub use server::*;

pub use detect::detect_engine;

use anyhow::{Context, Result};
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{
    AnyStream, ClusterResponse, ExecChanges, ExecChunk, ExecExit, ExecRequest, FileDelta,
    FileStamp, HandshakeResponse, LoadedWorkspaceInfo, NodeGossip, PathTranslator, PeerInfo,
    PlaceRequest, PlaceResponse, ProdCodeCodec, RemoteExecCommand, RemoteExecFormat,
    RemoteExecLanguage, RemoteExecRequest, RemoteExecResult, RemoteExecStream, RemoteExecTestEvent,
    ScrubSecrets, StatusResponse, SyncProbeRequest, SyncProbeResponse, SyncRequest, SyncResponse,
    WireMessage, content_hash, negotiate_protocol_version, parse_cargo_json_event,
    parse_go_test_json_event,
    path::{file_uri, uri_or_path},
};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio_util::codec::Framed;
use workspace::{SessionView, WorkspaceManager};


/// Whether an LSP message is a request from the server (an id and a method), not a notification.
fn is_server_request(json: &str) -> bool {
    json.contains("\"id\"")
        && serde_json::from_str::<serde_json::Value>(json)
            .is_ok_and(|v| v.get("id").is_some() && v.get("method").is_some())
}

fn fallback_answers_request(json: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(json).is_ok_and(|value| {
        value.get("id").is_some_and(|id| !id.is_null())
            && matches!(
                value.get("method").and_then(serde_json::Value::as_str),
                Some(
                    "window/workDoneProgress/create"
                        | "workspace/configuration"
                        | "client/registerCapability"
                )
            )
    })
}

/// Sends the client the note an engine attached to an answer given while its server was still
/// loading or indexing, just before the answer, and takes it off the answer (#391).
async fn send_busy_note(resp: &mut serde_json::Value, out_tx: &SharedOutputSender) {
    let Some(busy) = resp
        .as_object_mut()
        .and_then(|o| o.remove(prod_code_protocol::readiness::BUSY_MEMBER))
    else {
        return;
    };
    let note = serde_json::json!({
        "jsonrpc": "2.0",
        "method": prod_code_protocol::readiness::BUSY_NOTIFICATION,
        "params": busy
    });
    let _ = out_tx.send(WireMessage::LspPayload(note.to_string())).await;
}

/// A managed (out-of-process) language server the gateway can talk LSP to.
/// Default wall-clock limit for a remote command when the client does not set one.
const EXEC_DEFAULT_TIMEOUT_SECS: u64 = 3600;

/// Maximum supported timeout for remote execution (7 days) to prevent overflow in deadline arithmetic.
const MAX_REMOTE_EXEC_TIMEOUT_SECS: u64 = 86400 * 7;

/// Maximum line buffer size for parsing structured JSON/text test streams (1 MiB).
const MAX_JSON_LINE_BUFFER_BYTES: usize = 1024 * 1024;

/// Largest file whose old bytes a pre-command snapshot keeps, so that the gateway can put it
/// back when the command's changes never reach the client (#262). Source files are far smaller;
/// what is larger is mostly data a command regenerates anyway.
const RESTORE_MAX_FILE: u64 = 1024 * 1024;

/// Most bytes one pre-command snapshot keeps in memory. A command runs while its snapshot is
/// held, so a checkout with a large tree of small files must not cost the node gigabytes; a
/// file past the budget is marked stale when it has to be restored, and the client sends it.
const RESTORE_BUDGET: u64 = 256 * 1024 * 1024;

/// The old contents of a file, kept to restore it.
struct KeptFile {
    bytes: Vec<u8>,
    #[cfg(unix)]
    mode: u32,
}

/// What a workspace copy held before a remote command ran.
#[derive(Default)]
struct TreeSnapshot {
    /// Size and content hash of every file, for detecting what the command changed.
    stamps: std::collections::HashMap<String, (u64, u64)>,
    /// The bytes of every file up to [`RESTORE_MAX_FILE`], within [`RESTORE_BUDGET`].
    kept: std::collections::HashMap<String, KeptFile>,
}

/// Size and content hash of every file under `root` the sync layer cares about (build output
/// and VCS internals excluded).
fn stamp_tree(root: &std::path::Path) -> std::collections::HashMap<String, (u64, u64)> {
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

/// [`stamp_tree`] plus the bytes of the files small enough to keep, for detecting what a remote
/// command changed and for undoing it when the client cannot receive the changes.
fn snapshot_tree(root: &std::path::Path) -> TreeSnapshot {
    snapshot_tree_within(root, RESTORE_MAX_FILE, RESTORE_BUDGET)
}

/// [`snapshot_tree`] with its limits given, so that a test can exceed them cheaply.
fn snapshot_tree_within(root: &std::path::Path, max_file: u64, mut budget: u64) -> TreeSnapshot {
    let mut present = Vec::new();
    walk_files(root, root, &mut present);
    // Walked in a fixed order, so which files fit the budget does not depend on the directory
    // listing order.
    present.sort();
    let mut snapshot = TreeSnapshot::default();
    for (rel, path) in present {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let size = bytes.len() as u64;
        snapshot
            .stamps
            .insert(rel.clone(), (size, content_hash(&bytes)));
        if size > max_file || size > budget {
            continue;
        }
        budget -= size;
        #[cfg(unix)]
        let mode = {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(&path)
                .map(|m| m.permissions().mode())
                .unwrap_or(0o644)
        };
        snapshot.kept.insert(
            rel,
            KeptFile {
                bytes,
                #[cfg(unix)]
                mode,
            },
        );
    }
    snapshot
}

/// What [`restore_tree`] did to a workspace copy.
#[derive(Debug, Default)]
struct Restored {
    /// Every file put back as it was, created by the command and removed, or removed because
    /// its old bytes were not kept: what a warm engine must be told.
    files: Vec<FileDelta>,
    /// The files among them that could not be put back and are gone from the copy.
    stale: Vec<String>,
}

/// Undoes what a command changed in the workspace copy since `before`: a changed file gets its
/// old bytes back, a file the command created is removed, a file it deleted is recreated. A
/// changed or deleted file whose old bytes were not kept is removed and reported as stale, so
/// that the client sends its own version. The comparison is made here rather than with
/// [`changed_since`], which leaves out files above the size the client is sent: a restore must
/// not leave any of them behind.
///
/// `synced` holds the files a client sync delivered while the command ran, with the hash of the
/// text it wrote. Such a file is the checkout's newer text, not the command's, and stays; if the
/// command changed it again after it arrived, that text is gone, so the file is removed and
/// reported as stale like one whose bytes were not kept.
fn restore_tree(
    root: &std::path::Path,
    before: &TreeSnapshot,
    synced: &std::collections::HashMap<String, Option<u64>>,
) -> Restored {
    let after = stamp_tree(root);
    let mut restored = Restored::default();
    let mut touched: Vec<&String> = after
        .iter()
        .filter(|(rel, stamp)| before.stamps.get(*rel) != Some(*stamp))
        .map(|(rel, _)| rel)
        .chain(before.stamps.keys().filter(|rel| !after.contains_key(*rel)))
        .collect();
    touched.sort();
    for rel in touched {
        let target = root.join(rel);
        if let Some(synced_hash) = synced.get(rel) {
            if after.get(rel).map(|stamp| stamp.1) == *synced_hash {
                continue;
            }
            if target.exists() && std::fs::remove_file(&target).is_err() {
                continue;
            }
            prune_empty_parents(root, target.parent());
            restored.stale.push(rel.clone());
            restored.files.push(FileDelta {
                relative_path: rel.clone(),
                content: None,
                is_executable: false,
            });
            continue;
        }
        match before.kept.get(rel) {
            Some(kept) => {
                if let Some(parent) = target.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if let Err(e) = std::fs::write(&target, &kept.bytes) {
                    tracing::warn!(error = %e, file = %target.display(), "restoring a file failed");
                    continue;
                }
                #[cfg(unix)]
                let is_executable = {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(
                        &target,
                        std::fs::Permissions::from_mode(kept.mode),
                    );
                    kept.mode & 0o111 != 0
                };
                #[cfg(not(unix))]
                let is_executable = false;
                restored.files.push(FileDelta {
                    relative_path: rel.clone(),
                    content: Some(kept.bytes.clone()),
                    is_executable,
                });
            }
            None => {
                if target.exists() && std::fs::remove_file(&target).is_err() {
                    continue;
                }
                prune_empty_parents(root, target.parent());
                if before.stamps.contains_key(rel) {
                    restored.stale.push(rel.clone());
                }
                restored.files.push(FileDelta {
                    relative_path: rel.clone(),
                    content: None,
                    is_executable: false,
                });
            }
        }
    }
    restored
}

/// Puts the workspace copy back the way `before` found it when the changes a command made will
/// never reach its client (#262), and returns how many files were put back. Without this the
/// copy keeps changes the checkout does not have: the next sync sends only what changed locally,
/// so a later check would run on code nobody committed. The files that could not be put back
/// are recorded as stale for the next handshake or sync. The warm engines are told, like after
/// a sync, so that they see the old text again.
async fn restore_after_lost_client(
    workspace_manager: &WorkspaceManager,
    workspace: &std::path::Path,
    before: Arc<TreeSnapshot>,
    started: Instant,
) -> usize {
    let root = workspace.to_path_buf();
    let restored = tokio::task::spawn_blocking(move || {
        let synced = workspace::synced_since(&root, started);
        let restored = restore_tree(&root, &before, &synced);
        workspace::record_stale_paths(&root, &restored.stale);
        restored
    })
    .await
    .unwrap_or_default();
    let unkept = restored
        .files
        .iter()
        .filter(|f| f.content.is_none() && restored.stale.contains(&f.relative_path))
        .count();
    if unkept > 0 {
        tracing::warn!(
            workspace = %workspace.display(),
            stale = unkept,
            "🛠️ [EXEC] files a command changed were too large to keep; removed until the client sends them"
        );
    }
    refresh_engines(workspace_manager, workspace, &restored.files).await;
    restored
        .files
        .iter()
        .filter(|f| f.content.is_some())
        .count()
}

/// Files that differ between `before` and the tree now: new/changed ones with content,
/// removed ones as deletions. Files above 5 MiB are ignored.
fn changed_since(
    root: &std::path::Path,
    before: &std::collections::HashMap<String, (u64, u64)>,
) -> Vec<FileDelta> {
    const MAX_PULL_FILE: u64 = 5 * 1024 * 1024;
    let after = stamp_tree(root);
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

/// Tells a warm engine that the files a command just wrote are its new base, and drops every
/// engine under the workspace when one of them is a project manifest — what a sync does, for a
/// change that did not arrive as a sync.
///
/// A formatter or a generator rewrites files in the workspace copy; the client is sent the new
/// contents and records them as synced, so no later sync ever carries them here. An engine that
/// is not told keeps answering from the text it had before the command, and every position in a
/// file the command moved is off by however many lines it moved — silently, because the file it
/// is asked about is opened fresh by the client and only the *other* files come from its copy.
async fn refresh_engines(
    workspace_manager: &WorkspaceManager,
    server_workspace: &std::path::Path,
    files: &[FileDelta],
) {
    let loaded_rust = workspace_manager
        .get_loaded(server_workspace)
        .await
        .map(|ws| ws.mirrored_rust_engines())
        .unwrap_or_default();
    let mut project_config_changed = false;
    for delta in files {
        project_config_changed |= is_project_config_file(&delta.relative_path);
        let text = match &delta.content {
            Some(bytes) => match std::str::from_utf8(bytes) {
                Ok(text) => Some(text.to_string()),
                Err(_) => continue,
            },
            None => None,
        };
        let target = server_workspace.join(&delta.relative_path);
        for engine_lock in &loaded_rust {
            let mut engine = engine_lock.lock().await;
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                engine.update_base(&target, text.clone())
            }));
            match res {
                Ok(Err(e)) => {
                    tracing::warn!(error = %e, file = %target.display(), "engine update after a command failed");
                }
                Err(panic) => {
                    let msg = if let Some(s) = panic.downcast_ref::<&str>() {
                        s.to_string()
                    } else if let Some(s) = panic.downcast_ref::<String>() {
                        s.clone()
                    } else {
                        "unknown panic".to_string()
                    };
                    tracing::warn!(panic = %msg, file = %target.display(), "engine update after a command panicked; continuing");
                }
                Ok(Ok(())) => {}
            }
        }
    }
    if project_config_changed {
        let dropped = workspace_manager.unload_under(server_workspace).await;
        if dropped > 0 {
            tracing::info!(
                dropped,
                "a command changed the project configuration; engines reload on next session"
            );
        }
    }
}

/// Kills a tokio child and everything it spawned (its process group), then the child itself:
/// shadow runs, which let tokio reap their children.
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
/// The environment that lets every git worktree of a project share one C/C++ compiler cache
/// (#243). Each worktree has its own server copy at its own path, and a compiler cache keyed by
/// absolute paths shares nothing between them: the fmt library built in a second copy took
/// 22.9 s with sccache as the launcher (2 hits of 114 compiles) and 0.71 s with ccache and
/// `CCACHE_BASEDIR` set to the copy, which ccache reads on every compile. sccache reads its
/// base directories once, when its server starts, so it cannot follow worktrees that appear
/// later. CMake picks the launchers up when it configures a build directory. Nothing when the
/// node has no ccache; the caller's own variables are applied after these and win.
pub fn compiler_cache_env(workspace: &Path, ccache: bool) -> Vec<(String, String)> {
    if !ccache {
        return Vec::new();
    }
    vec![
        (
            "CCACHE_BASEDIR".to_string(),
            workspace.to_string_lossy().into_owned(),
        ),
        ("CCACHE_NOHASHDIR".to_string(), "1".to_string()),
        (
            "CCACHE_SLOPPINESS".to_string(),
            "pch_defines,time_macros".to_string(),
        ),
        ("CCACHE_PCH_EXTSUM".to_string(), "1".to_string()),
        (
            "CMAKE_C_COMPILER_LAUNCHER".to_string(),
            "ccache".to_string(),
        ),
        (
            "CMAKE_CXX_COMPILER_LAUNCHER".to_string(),
            "ccache".to_string(),
        ),
    ]
}

/// Polyglot compiler and build cache environment across Rust, Go, Python, Node, C/C++, and Swift (Roadmap 6.2, 3.4, 3.7).
pub fn polyglot_compiler_cache_env(
    workspace: &Path,
    ccache: bool,
    ram_target_dir: Option<&Path>,
) -> Vec<(String, String)> {
    let mut env = compiler_cache_env(workspace, ccache);
    if let Some(target) = ram_target_dir {
        env.push((
            "CARGO_TARGET_DIR".to_string(),
            target.to_string_lossy().into_owned(),
        ));
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home_path = PathBuf::from(home);
        let gocache = home_path.join(".cache/go-build");
        let gomodcache = home_path.join("go/pkg/mod");
        if gocache.is_dir() {
            env.push((
                "GOCACHE".to_string(),
                gocache.to_string_lossy().into_owned(),
            ));
        }
        if gomodcache.is_dir() {
            env.push((
                "GOMODCACHE".to_string(),
                gomodcache.to_string_lossy().into_owned(),
            ));
        }
        let uv_cache = home_path.join(".cache/uv");
        if uv_cache.is_dir() {
            env.push((
                "UV_CACHE_DIR".to_string(),
                uv_cache.to_string_lossy().into_owned(),
            ));
        }
        let pip_cache = home_path.join(".cache/pip");
        if pip_cache.is_dir() {
            env.push((
                "PIP_CACHE_DIR".to_string(),
                pip_cache.to_string_lossy().into_owned(),
            ));
        }
        let pnpm_store = home_path.join(".local/share/pnpm/store");
        if pnpm_store.is_dir() {
            env.push((
                "npm_config_store_dir".to_string(),
                pnpm_store.to_string_lossy().into_owned(),
            ));
        }
        let npm_cache = home_path.join(".npm");
        if npm_cache.is_dir() {
            env.push((
                "npm_config_cache".to_string(),
                npm_cache.to_string_lossy().into_owned(),
            ));
        }
        let yarn_cache = home_path.join(".cache/yarn");
        if yarn_cache.is_dir() {
            env.push((
                "YARN_CACHE_FOLDER".to_string(),
                yarn_cache.to_string_lossy().into_owned(),
            ));
        }
    }
    // Python shared virtual-environment stub cache across worktrees (Roadmap 3.6)
    env.extend(python_cache::python_stub_cache_env_for_workspace(workspace));
    // Swift shared module cache across worktrees (Roadmap 3.7)
    env.extend(swift_cache::swift_module_cache_env());
    // TypeScript shared @types and declaration cache across worktrees (Roadmap 3.5)
    env.extend(ts_cache::ts_types_cache_env());
    env
}

pub fn is_ram_cache_enabled_with(enabled: bool, env_val: Option<&str>) -> bool {
    enabled
        || env_val
            .map(|v| {
                matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
            .unwrap_or(false)
}

pub fn is_ram_cache_enabled(enabled: bool) -> bool {
    let env = std::env::var("PROD_CODE_BUILD_RAM").ok();
    is_ram_cache_enabled_with(enabled, env.as_deref())
}

/// Resolves or initializes an isolated in-memory RAM-disk build cache for `workspace` (Roadmap 6.2).
/// Returns `Some(PathBuf)` if enabled and headroom permits (>= 20% free and >= 256 MiB free); otherwise `None`.
pub fn resolve_ram_build_cache(
    workspace: &Path,
    enabled: bool,
    custom_dir: Option<&Path>,
) -> Option<PathBuf> {
    if !is_ram_cache_enabled(enabled) {
        return None;
    }
    let default_shm = Path::new("/dev/shm/prod-code-build");
    let fallback_tmp = Path::new("/tmp/prod-code-build");
    let base_dir = custom_dir.unwrap_or_else(|| {
        if Path::new("/dev/shm").is_dir() {
            default_shm
        } else {
            fallback_tmp
        }
    });

    if let Some(space) = disk_space(base_dir) {
        let free_share = space.free as f64 / space.total.max(1) as f64;
        const MIN_FREE_RAM_SHARE: f64 = 0.20;
        const MIN_FREE_BYTES: u64 = 256 * 1024 * 1024;
        if free_share < MIN_FREE_RAM_SHARE || space.free < MIN_FREE_BYTES {
            tracing::info!(
                dir = %base_dir.display(),
                free_mb = space.free / (1024 * 1024),
                "🌱 [BUILD_RAM] insufficient RAM disk headroom; falling back to disk cache"
            );
            return None;
        }
    }

    let workspace_name = workspace.file_name()?.to_str()?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(&workspace, &mut hasher);
    let hash = std::hash::Hasher::finish(&hasher);
    let ws_cache_dir = base_dir.join(format!("{workspace_name}-{hash:016x}"));
    let target_dir = ws_cache_dir.join("target");
    if let Err(e) = std::fs::create_dir_all(&target_dir) {
        tracing::warn!(%e, dir = %target_dir.display(), "failed to create RAM build cache dir; falling back to disk");
        return None;
    }
    let marker = ws_cache_dir.join(".last_used");
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&marker);
    Some(target_dir)
}

/// An RAII lease that marks a RAM-disk cache directory as actively in use by a running build.
///
/// On Unix, an advisory flock is held on the marker file for the entire lifetime of the lease.
/// If the gateway process is killed or crashes, the OS kernel automatically closes the file
/// descriptor and releases the lock, allowing sweepers to identify and clean stale markers.
pub struct RamBuildLease {
    marker: Option<PathBuf>,
    #[cfg(unix)]
    _lock_file: Option<std::fs::File>,
}

impl RamBuildLease {
    pub fn acquire(target_dir: &Path) -> Self {
        if let Some(ws_cache_dir) = target_dir.parent() {
            let lease_id = NEXT_COMMAND_ID.fetch_add(1, Ordering::Relaxed);
            let pid = std::process::id();
            let marker = ws_cache_dir.join(format!(".active_{}_{}", pid, lease_id));
            #[cfg(unix)]
            {
                use std::io::Write;
                use std::os::unix::io::AsRawFd;
                if let Ok(mut file) = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .open(&marker)
                {
                    let ret =
                        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
                    if ret == 0 {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0);
                        let meta = format!(
                            "{{\"pid\":{},\"lease_id\":{},\"created_at\":{}}}\n",
                            pid, lease_id, now
                        );
                        let _ = file.write_all(meta.as_bytes());
                        let _ = file.flush();
                        return Self {
                            marker: Some(marker),
                            _lock_file: Some(file),
                        };
                    }
                }
            }
            #[cfg(not(unix))]
            {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let meta = format!(
                    "{{\"pid\":{},\"lease_id\":{},\"created_at\":{}}}\n",
                    pid, lease_id, now
                );
                if let Ok(()) = std::fs::write(&marker, meta.as_bytes()) {
                    return Self {
                        marker: Some(marker),
                    };
                }
            }
        }
        Self {
            marker: None,
            #[cfg(unix)]
            _lock_file: None,
        }
    }
}

impl Drop for RamBuildLease {
    fn drop(&mut self) {
        if let Some(ref path) = self.marker {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Checks whether a RAM-disk lease marker represents an active build process.
///
/// On Unix, an advisory flock is held for the lifetime of a live lease. If the process has died or crashed,
/// flock acquisition succeeds; this function unlinks the stale marker and returns `false`.
/// If the lock cannot be acquired because a running process is holding it, returns `true`.
pub fn is_ram_lease_active(marker: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        if let Ok(file) = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(marker)
        {
            let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            if ret == 0 {
                // Successfully locked: owning process died or exited without dropping the lease.
                // Remove the stale marker file while holding the lock.
                let _ = std::fs::remove_file(marker);
                false
            } else {
                // Lock busy: active build process holds this lease.
                true
            }
        } else {
            // Already unlinked or cannot open
            false
        }
    }
    #[cfg(not(unix))]
    {
        if let Ok(meta) = marker.metadata() {
            if let Ok(elapsed) = meta.modified().and_then(|m| m.elapsed()) {
                if elapsed.as_secs() > 7200 {
                    let _ = std::fs::remove_file(marker);
                    return false;
                }
            }
        }
        true
    }
}

/// Sweeps stale or orphaned RAM-disk build caches on startup or periodic maintenance.
pub fn sweep_ram_build_caches(base_dir: &Path) -> usize {
    if !base_dir.is_dir() {
        return 0;
    }
    let running: std::collections::HashSet<String> = RUNNING_COMMANDS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .values()
        .map(|(ws, _, _)| ws.clone())
        .collect();

    let mut removed = 0;
    if let Ok(entries) = std::fs::read_dir(base_dir) {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                let path = entry.path();
                let dir_name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default();
                let is_running = running.iter().any(|ws| dir_name.starts_with(ws));
                if is_running {
                    continue;
                }
                if let Ok(children) = std::fs::read_dir(&path) {
                    let mut has_active_lease = false;
                    for child in children.flatten() {
                        let name = child.file_name();
                        let name_str = name.to_str().unwrap_or_default();
                        if name_str.starts_with(".active_") {
                            if is_ram_lease_active(&child.path()) {
                                has_active_lease = true;
                            }
                        }
                    }
                    if has_active_lease {
                        continue;
                    }
                }
                let marker = path.join(".last_used");
                let metadata_target = if marker.is_file() {
                    marker.metadata().ok()
                } else {
                    entry.metadata().ok()
                };
                if let Some(meta) = metadata_target {
                    let is_old = meta
                        .modified()
                        .ok()
                        .and_then(|m| m.elapsed().ok())
                        .map(|age| age.as_secs() > 86400)
                        .unwrap_or(false);
                    if is_old {
                        if let Ok(()) = std::fs::remove_dir_all(&path) {
                            removed += 1;
                        }
                    }
                }
            }
        }
    }
    if removed > 0 {
        tracing::info!(dir = %base_dir.display(), removed, "swept old RAM-disk build caches");
    }
    removed
}

/// Whether `program` is an executable file in a directory of `PATH`.
fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

pub async fn run_exec(
    storage_root: &std::path::Path,
    metrics: &metrics::Metrics,
    workspace_manager: &WorkspaceManager,
    framed: &mut Framed<AnyStream, ProdCodeCodec>,
    req: ExecRequest,
) -> Result<()> {
    run_exec_with_ram(
        storage_root,
        metrics,
        workspace_manager,
        framed,
        req,
        false,
        None,
    )
    .await
}

pub async fn run_exec_with_ram(
    storage_root: &std::path::Path,
    metrics: &metrics::Metrics,
    workspace_manager: &WorkspaceManager,
    framed: &mut Framed<AnyStream, ProdCodeCodec>,
    req: ExecRequest,
    build_cache_ram: bool,
    build_cache_dir: Option<&std::path::Path>,
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
        usage: None,
        platform: Some(prod_code_protocol::platform()),
    };
    if !workspace.is_dir() {
        framed
            .send(WireMessage::ExecExit(fail(format!(
                "workspace {workspace_str} is not synced to this gateway"
            ))))
            .await?;
        return Ok(());
    }
    workspace::touch_last_used(&workspace);
    let Some((program, args)) = req.command.split_first() else {
        framed
            .send(WireMessage::ExecExit(fail("empty command".to_string())))
            .await?;
        return Ok(());
    };

    let timeout_secs = if req.timeout_secs == 0 {
        EXEC_DEFAULT_TIMEOUT_SECS
    } else {
        req.timeout_secs
    };
    if timeout_secs > MAX_REMOTE_EXEC_TIMEOUT_SECS {
        framed
            .send(WireMessage::ExecExit(fail(format!(
                "timeout_secs ({timeout_secs}) exceeds maximum allowed ({MAX_REMOTE_EXEC_TIMEOUT_SECS})"
            ))))
            .await?;
        return Ok(());
    }
    let timeout = std::time::Duration::from_secs(timeout_secs);
    let deadline = match tokio::time::Instant::now().checked_add(timeout) {
        Some(d) => d,
        None => {
            framed
                .send(WireMessage::ExecExit(fail(
                    "timeout_secs overflowed deadline calculation".to_string(),
                )))
                .await?;
            return Ok(());
        }
    };
    if let Some((free, total)) = workspace::free_and_total_bytes(&workspace)
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
            framed
                .send(WireMessage::ExecExit(fail(format!(
                    "refused: node {hostname} has only {free_gb:.1} GB free on {} ({used_pct:.0}% full)",
                    workspace.display()
                ))))
                .await?;
            return Ok(());
        }
        if free_gb < 10.0 {
            let warn = format!(
                "[prod-code exec] WARNING: {free_gb:.1} GB free on node {hostname} ({used_pct:.0}% used)\n"
            );
            let _ = framed
                .send(WireMessage::ExecChunk(ExecChunk {
                    stderr: true,
                    data: Some(warn.into_bytes()),
                }))
                .await;
        }
    }
    // Syncs that land after this are the client's newer text, which a restore leaves alone.
    let snapshot_started = Instant::now();
    let before = Arc::new(if req.pull_changes {
        let root = workspace.clone();
        tokio::task::spawn_blocking(move || snapshot_tree(&root))
            .await
            .unwrap_or_default()
    } else {
        TreeSnapshot::default()
    });

    let run_dir = match req.subdir.as_deref() {
        Some(sub)
            if !sub.is_empty() && !sub.starts_with('/') && !sub.split('/').any(|c| c == "..") =>
        {
            let target = workspace.join(sub);
            if !target.is_dir() {
                framed
                    .send(WireMessage::ExecExit(fail(format!(
                        "working directory '{sub}' does not exist in workspace {workspace_str}"
                    ))))
                    .await?;
                return Ok(());
            }
            target
        }
        Some(sub) if !sub.is_empty() => {
            framed
                .send(WireMessage::ExecExit(fail(format!(
                    "invalid working directory '{sub}'"
                ))))
                .await?;
            return Ok(());
        }
        _ => workspace.clone(),
    };
    // A std child, reaped here with `wait4` so its resource use comes back with the exit
    // status (#180); tokio only gets the pipes. The gateway binary starts it through its exec
    // shim, so that the peak memory reported is the command's and not the gateway's (#255).
    let ram_target = resolve_ram_build_cache(&workspace, build_cache_ram, build_cache_dir);
    let _ram_lease = ram_target.as_deref().map(RamBuildLease::acquire);
    let (mut cmd, report) = exec_shim::command(program);
    cmd.args(args)
        .current_dir(&run_dir)
        .envs(polyglot_compiler_cache_env(
            &workspace,
            on_path("ccache"),
            ram_target.as_deref(),
        ))
        .envs(req.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    // The cluster's secret credentials (token and TLS material) are never given to executed commands (#402, Phase 5.6).
    // Scrubbed AFTER request env is applied so that client requests cannot inject or read cluster secrets.
    cmd.scrub_cluster_secrets();
    // Own process group, so a timeout or client disconnect can take down the whole tree
    // (cargo -> test binary -> its helpers), not just the direct child.
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
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
    let _running = RunningEntry::start(&workspace, &req.command);

    // rapidfire (lock-free MPSC): stdout and stderr readers fan in, the session task drains.
    let (tx, mut rx) = rapidfire::mpsc::bounded::<ExecChunk>(256);
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
    drop(tx);
    // Reaped on a blocking thread; the pid stays ours to kill until then.
    let pid = child.id();
    let exited = Arc::new(std::sync::Mutex::new(false));
    let (exit_tx, mut exit_rx) = tokio::sync::oneshot::channel();
    {
        let exited = exited.clone();
        tokio::task::spawn_blocking(move || {
            let _ = exit_tx.send(wait_with_usage(pid as i32, &exited));
            drop(child);
        });
    }

    let mut timed_out = false;
    let mut status = None;
    let mut chunks_open = true;
    let mut client_left = false;
    loop {
        tokio::select! {
            chunk = rx.recv(), if chunks_open => match chunk {
                // A client that can no longer be written to is gone as surely as one that
                // hung up, and the command must not outlive it either way.
                Ok(chunk) => client_left = framed.send(WireMessage::ExecChunk(chunk)).await.is_err(),
                Err(_) => chunks_open = false,
            },
            exit = &mut exit_rx, if status.is_none() => {
                status = Some(exit.ok().flatten());
            }
            _ = tokio::time::sleep_until(deadline), if !timed_out && status.is_none() => {
                timed_out = true;
                kill_exec_group(pid, &exited);
            }
            incoming = framed.next(), if status.is_none() => match incoming {
                Some(Ok(WireMessage::Ping)) => {
                    client_left = framed.send(WireMessage::Pong).await.is_err();
                }
                // A connection that fails is as gone as one that closed: nothing the command
                // changes can reach the client anymore.
                Some(Ok(WireMessage::Disconnect { .. })) | Some(Err(_)) | None => client_left = true,
                _ => {}
            }
        }
        if client_left || (!chunks_open && status.is_some()) {
            break;
        }
    }
    if client_left {
        kill_exec_group(pid, &exited);
        // Readers blocked on a full channel see it close and let go of the pipes.
        drop(rx);
        // The command has to be gone before its changes are undone, or it could write again
        // after the restore.
        if status.is_none() {
            let _ = exit_rx.await;
        }
        if req.pull_changes {
            let restored =
                restore_after_lost_client(workspace_manager, &workspace, before, snapshot_started)
                    .await;
            tracing::info!(
                workspace = %workspace_str,
                "🛠️ [EXEC] client left; command killed; {restored} file(s) it changed restored"
            );
        } else {
            tracing::info!(workspace = %workspace_str, "🛠️ [EXEC] client left; command killed");
        }
        return Ok(());
    }
    for reader in readers {
        let _ = reader.await;
    }
    // The shim's report describes the command itself. There is none when the group was killed
    // on a timeout, and then what `wait4` said about the shim stands in for it.
    let report_status = report.as_ref().and_then(exec_shim::ReportFile::read);
    let shim_raw_status = status.flatten();
    let had_report = report.is_some();
    drop(report);
    let (exit_code, usage, exec_err) = if let Some((raw, usage)) = report_status {
        use std::os::unix::process::ExitStatusExt;
        (
            std::process::ExitStatus::from_raw(raw).code(),
            Some(usage),
            None,
        )
    } else if let Some((raw, usage)) = shim_raw_status {
        use std::os::unix::process::ExitStatusExt;
        let exit_status = std::process::ExitStatus::from_raw(raw);
        if exit_status.code() == Some(74) && had_report && !timed_out {
            (
                Some(74),
                Some(usage),
                Some(
                    "exec shim failed to write process report (disk full or write error)"
                        .to_string(),
                ),
            )
        } else {
            (exit_status.code(), Some(usage), None)
        }
    } else {
        (None, None, None)
    };
    if exit_code == Some(254) {
        tracing::warn!(
            "exec command exited with 254; ensuring sccache server is running cleanly on host"
        );
        tokio::task::spawn_blocking(crate::shadow::ensure_sccache_server)
            .await
            .ok();
    }
    let duration_ms = start.elapsed().as_millis() as u64;
    tracing::info!(
        workspace = %workspace_str,
        command = %req.command.join(" "),
        exit_code = ?exit_code,
        timed_out,
        duration_ms,
        "🛠️ [EXEC] finished"
    );
    {
        let mut ev = metrics::Event::blank("exec");
        ev.agent = req
            .client_agent
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        ev.host = req
            .client_host
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        ev.workspace = workspace
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        ev.command = req.command.join(" ");
        ev.duration_ms = start.elapsed().as_millis() as u64;
        ev.exit_code = exit_code;
        ev.ok = exit_code == Some(0) && exec_err.is_none();
        metrics.record(ev);
    }
    if req.pull_changes {
        let (root, snapshot) = (workspace.clone(), Arc::clone(&before));
        let files = tokio::task::spawn_blocking(move || changed_since(&root, &snapshot.stamps))
            .await
            .unwrap_or_default();
        if !files.is_empty() {
            tracing::info!(
                workspace = %workspace_str,
                files = files.len(),
                "🛠️ [EXEC] sending back files the command changed"
            );
            if let Err(e) = framed
                .send(WireMessage::ExecChanges(ExecChanges {
                    files: files.clone(),
                }))
                .await
            {
                // The client never receives these changes, so the copy must not keep them.
                let restored = restore_after_lost_client(
                    workspace_manager,
                    &workspace,
                    before,
                    snapshot_started,
                )
                .await;
                tracing::info!(
                    workspace = %workspace_str,
                    "🛠️ [EXEC] client left before the changes were sent; {restored} file(s) the command changed restored"
                );
                return Err(e.into());
            }
            refresh_engines(workspace_manager, &workspace, &files).await;
        }
    }
    framed
        .send(WireMessage::ExecExit(ExecExit {
            exit_code,
            duration_ms,
            server_workspace_root: workspace_str,
            timed_out,
            error: exec_err,
            usage,
            platform: Some(prod_code_protocol::platform()),
        }))
        .await?;
    Ok(())
}

pub async fn run_remote_exec(
    storage_root: &std::path::Path,
    metrics: &metrics::Metrics,
    workspace_manager: &WorkspaceManager,
    framed: &mut Framed<AnyStream, ProdCodeCodec>,
    req: RemoteExecRequest,
) -> Result<()> {
    run_remote_exec_with_ram(
        storage_root,
        metrics,
        workspace_manager,
        framed,
        req,
        false,
        None,
    )
    .await
}

pub async fn run_remote_exec_with_ram(
    storage_root: &std::path::Path,
    metrics: &metrics::Metrics,
    workspace_manager: &WorkspaceManager,
    framed: &mut Framed<AnyStream, ProdCodeCodec>,
    req: RemoteExecRequest,
    build_cache_ram: bool,
    build_cache_dir: Option<&std::path::Path>,
) -> Result<()> {
    use tokio::io::AsyncReadExt;

    let start = Instant::now();
    let workspace = workspace::server_workspace_path(
        storage_root,
        &req.client_workspace_root,
        req.base_workspace_name.as_deref(),
    );
    let workspace_str = workspace.to_string_lossy().to_string();
    let fail = |error: String| RemoteExecResult {
        exit_code: None,
        duration_ms: 0,
        server_workspace_root: workspace_str.clone(),
        timed_out: false,
        error: Some(error),
        usage: None,
        platform: Some(prod_code_protocol::platform()),
        diagnostics: Vec::new(),
        tests_passed: 0,
        tests_failed: 0,
        tests_skipped: 0,
        test_failures: Vec::new(),
        benches: Vec::new(),
    };

    if !workspace.is_dir() {
        framed
            .send(WireMessage::RemoteExecResult(fail(format!(
                "workspace {workspace_str} is not synced to this gateway"
            ))))
            .await?;
        return Ok(());
    }
    workspace::touch_last_used(&workspace);

    let argv = req.to_argv();
    let Some((program, args)) = argv.split_first() else {
        framed
            .send(WireMessage::RemoteExecResult(fail(
                "empty command".to_string(),
            )))
            .await?;
        return Ok(());
    };

    let timeout_secs = if req.timeout_secs == 0 {
        EXEC_DEFAULT_TIMEOUT_SECS
    } else {
        req.timeout_secs
    };
    if timeout_secs > MAX_REMOTE_EXEC_TIMEOUT_SECS {
        framed
            .send(WireMessage::RemoteExecResult(fail(format!(
                "timeout_secs ({timeout_secs}) exceeds maximum allowed ({MAX_REMOTE_EXEC_TIMEOUT_SECS})"
            ))))
            .await?;
        return Ok(());
    }
    let timeout = std::time::Duration::from_secs(timeout_secs);
    let deadline = match tokio::time::Instant::now().checked_add(timeout) {
        Some(d) => d,
        None => {
            framed
                .send(WireMessage::RemoteExecResult(fail(
                    "timeout_secs overflowed deadline calculation".to_string(),
                )))
                .await?;
            return Ok(());
        }
    };

    if let Some((free, total)) = workspace::free_and_total_bytes(&workspace)
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
            framed
                .send(WireMessage::RemoteExecResult(fail(format!(
                    "refused: node {hostname} has only {free_gb:.1} GB free on {} ({used_pct:.0}% full)",
                    workspace.display()
                ))))
                .await?;
            return Ok(());
        }
        if free_gb < 10.0 {
            let warn = format!(
                "[prod-code exec] WARNING: {free_gb:.1} GB free on node {hostname} ({used_pct:.0}% used)\n"
            );
            let _ = framed
                .send(WireMessage::RemoteExecStream(RemoteExecStream::Chunk(
                    ExecChunk {
                        stderr: true,
                        data: Some(warn.into_bytes()),
                    },
                )))
                .await;
        }
    }

    let snapshot_started = Instant::now();
    let before = Arc::new(if req.pull_changes {
        let root = workspace.clone();
        tokio::task::spawn_blocking(move || snapshot_tree(&root))
            .await
            .unwrap_or_default()
    } else {
        TreeSnapshot::default()
    });

    let run_dir = match req.subdir.as_deref() {
        Some(sub)
            if !sub.is_empty()
                && !sub.starts_with('/')
                && !sub.split('/').any(|c| c == "..")
                && workspace.join(sub).is_dir() =>
        {
            workspace.join(sub)
        }
        _ => workspace.clone(),
    };

    let ram_target = resolve_ram_build_cache(&workspace, build_cache_ram, build_cache_dir);
    let _ram_lease = ram_target.as_deref().map(RamBuildLease::acquire);
    let (mut cmd, report) = exec_shim::command(program);
    cmd.args(args)
        .current_dir(&run_dir)
        .envs(polyglot_compiler_cache_env(
            &workspace,
            on_path("ccache"),
            ram_target.as_deref(),
        ))
        .envs(req.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    cmd.scrub_cluster_secrets();
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            framed
                .send(WireMessage::RemoteExecResult(fail(format!(
                    "failed to start {program}: {e}"
                ))))
                .await?;
            return Ok(());
        }
    };

    tracing::info!(
        workspace = %workspace_str,
        command = %argv.join(" "),
        language = ?req.language,
        "🛠️ [REMOTE_EXEC] started"
    );
    let _running = RunningEntry::start(&workspace, &argv);

    let (tx, mut rx) = rapidfire::mpsc::bounded::<ExecChunk>(256);
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
    drop(tx);

    let pid = child.id();
    let exited = Arc::new(std::sync::Mutex::new(false));
    let (exit_tx, mut exit_rx) = tokio::sync::oneshot::channel();
    {
        let exited = exited.clone();
        tokio::task::spawn_blocking(move || {
            let _ = exit_tx.send(wait_with_usage(pid as i32, &exited));
            drop(child);
        });
    }

    let mut timed_out = false;
    let mut status = None;
    let mut chunks_open = true;
    let mut client_left = false;

    let mut stdout_line_buf = String::new();
    let mut diagnostics = Vec::new();
    let mut tests_passed = 0;
    let mut tests_failed = 0;
    let mut tests_skipped = 0;
    let mut test_failures = Vec::new();
    let mut benches = Vec::new();

    loop {
        tokio::select! {
            chunk = rx.recv(), if chunks_open => match chunk {
                Ok(chunk) => {
                    let is_stderr = chunk.stderr;
                    let chunk_data = chunk.data.clone();
                    if framed.send(WireMessage::RemoteExecStream(RemoteExecStream::Chunk(chunk))).await.is_err() {
                        client_left = true;
                    } else if (req.format == RemoteExecFormat::Json || matches!(req.command, RemoteExecCommand::Test | RemoteExecCommand::Bench)) && !is_stderr
                        && let Some(bytes) = chunk_data
                            && let Ok(text) = std::str::from_utf8(&bytes) {
                                stdout_line_buf.push_str(text);
                                while let Some(pos) = stdout_line_buf.find('\n') {
                                    if pos > MAX_JSON_LINE_BUFFER_BYTES {
                                        tracing::warn!(
                                            line_len = pos,
                                            "stdout line exceeded limit of {MAX_JSON_LINE_BUFFER_BYTES} bytes; dropping unparsed line"
                                        );
                                        stdout_line_buf.drain(..=pos);
                                        continue;
                                    }
                                    let line = stdout_line_buf[..pos].trim_end().to_string();
                                    stdout_line_buf.drain(..=pos);
                                    if line.is_empty() {
                                        continue;
                                    }
                                    let stream_event = match req.language {
                                        RemoteExecLanguage::Rust => parse_cargo_json_event(&line),
                                        RemoteExecLanguage::Go => parse_go_test_json_event(&line),
                                        _ => None,
                                    };
                                    if let Some(ev) = stream_event {
                                        match &ev {
                                            RemoteExecStream::Diagnostic(diag) => {
                                                diagnostics.push(diag.clone());
                                            }
                                            RemoteExecStream::TestEvent(test_ev) => match test_ev {
                                                RemoteExecTestEvent::Passed { .. } => tests_passed += 1,
                                                RemoteExecTestEvent::Failed { .. } => {
                                                    tests_failed += 1;
                                                    test_failures.push(test_ev.clone());
                                                }
                                                RemoteExecTestEvent::Skipped { .. } => tests_skipped += 1,
                                                RemoteExecTestEvent::Bench { .. } => benches.push(test_ev.clone()),
                                                _ => {}
                                            },
                                            _ => {}
                                        }
                                        if framed.send(WireMessage::RemoteExecStream(ev)).await.is_err() {
                                            client_left = true;
                                            break;
                                        }
                                    }
                                }
                                if stdout_line_buf.len() > MAX_JSON_LINE_BUFFER_BYTES {
                                    tracing::warn!(
                                        buf_len = stdout_line_buf.len(),
                                        "stdout buffer without newline exceeded limit of {MAX_JSON_LINE_BUFFER_BYTES} bytes; dropping unparsed buffer"
                                    );
                                    stdout_line_buf.clear();
                                }
                            }
                }
                Err(_) => chunks_open = false,
            },
            exit = &mut exit_rx, if status.is_none() => {
                status = Some(exit.ok().flatten());
            }
            _ = tokio::time::sleep_until(deadline), if !timed_out && status.is_none() => {
                timed_out = true;
                kill_exec_group(pid, &exited);
            }
            incoming = framed.next(), if status.is_none() => match incoming {
                Some(Ok(WireMessage::Ping)) => {
                    client_left = framed.send(WireMessage::Pong).await.is_err();
                }
                Some(Ok(WireMessage::Disconnect { .. })) | Some(Err(_)) | None => client_left = true,
                _ => {}
            }
        }
        if client_left || (!chunks_open && status.is_some()) {
            break;
        }
    }

    if client_left {
        kill_exec_group(pid, &exited);
        drop(rx);
        if status.is_none() {
            let _ = exit_rx.await;
        }
        if req.pull_changes {
            let restored =
                restore_after_lost_client(workspace_manager, &workspace, before, snapshot_started)
                    .await;
            tracing::info!(
                workspace = %workspace_str,
                "🛠️ [REMOTE_EXEC] client left; command killed; {restored} file(s) it changed restored"
            );
        } else {
            tracing::info!(workspace = %workspace_str, "🛠️ [REMOTE_EXEC] client left; command killed");
        }
        return Ok(());
    }

    for reader in readers {
        let _ = reader.await;
    }

    if (req.format == RemoteExecFormat::Json
        || matches!(
            req.command,
            RemoteExecCommand::Test | RemoteExecCommand::Bench
        ))
        && !stdout_line_buf.is_empty()
    {
        let line = stdout_line_buf.trim_end().to_string();
        if !line.is_empty() && line.len() <= MAX_JSON_LINE_BUFFER_BYTES {
            let stream_event = match req.language {
                RemoteExecLanguage::Rust => parse_cargo_json_event(&line),
                RemoteExecLanguage::Go => parse_go_test_json_event(&line),
                _ => None,
            };
            if let Some(ev) = stream_event {
                match &ev {
                    RemoteExecStream::Diagnostic(diag) => diagnostics.push(diag.clone()),
                    RemoteExecStream::TestEvent(test_ev) => match test_ev {
                        RemoteExecTestEvent::Passed { .. } => tests_passed += 1,
                        RemoteExecTestEvent::Failed { .. } => {
                            tests_failed += 1;
                            test_failures.push(test_ev.clone());
                        }
                        RemoteExecTestEvent::Skipped { .. } => tests_skipped += 1,
                        RemoteExecTestEvent::Bench { .. } => benches.push(test_ev.clone()),
                        _ => {}
                    },
                    _ => {}
                }
            }
        }
    }

    let report_status = report.as_ref().and_then(exec_shim::ReportFile::read);
    let shim_raw_status = status.flatten();
    let had_report = report.is_some();
    drop(report);
    let (exit_code, usage, exec_err) = if let Some((raw, usage)) = report_status {
        use std::os::unix::process::ExitStatusExt;
        (
            std::process::ExitStatus::from_raw(raw).code(),
            Some(usage),
            None,
        )
    } else if let Some((raw, usage)) = shim_raw_status {
        use std::os::unix::process::ExitStatusExt;
        let exit_status = std::process::ExitStatus::from_raw(raw);
        if exit_status.code() == Some(74) && had_report && !timed_out {
            (
                Some(74),
                Some(usage),
                Some(
                    "exec shim failed to write process report (disk full or write error)"
                        .to_string(),
                ),
            )
        } else {
            (exit_status.code(), Some(usage), None)
        }
    } else {
        (None, None, None)
    };

    if exit_code == Some(254) {
        tokio::task::spawn_blocking(crate::shadow::ensure_sccache_server)
            .await
            .ok();
    }

    let duration_ms = start.elapsed().as_millis() as u64;
    tracing::info!(
        workspace = %workspace_str,
        command = %argv.join(" "),
        exit_code = ?exit_code,
        timed_out,
        duration_ms,
        "🛠️ [REMOTE_EXEC] finished"
    );

    {
        let mut ev = metrics::Event::blank("remote_exec");
        ev.agent = req
            .client_agent
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        ev.host = req
            .client_host
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        ev.workspace = workspace
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        ev.command = argv.join(" ");
        ev.duration_ms = duration_ms;
        ev.exit_code = exit_code;
        ev.ok = exit_code == Some(0) && exec_err.is_none();
        metrics.record(ev);
    }

    if req.pull_changes {
        let (root, snapshot) = (workspace.clone(), Arc::clone(&before));
        let files = tokio::task::spawn_blocking(move || changed_since(&root, &snapshot.stamps))
            .await
            .unwrap_or_default();
        if !files.is_empty() {
            if let Err(e) = framed
                .send(WireMessage::ExecChanges(ExecChanges {
                    files: files.clone(),
                }))
                .await
            {
                let restored = restore_after_lost_client(
                    workspace_manager,
                    &workspace,
                    before,
                    snapshot_started,
                )
                .await;
                tracing::info!(
                    workspace = %workspace_str,
                    "🛠️ [REMOTE_EXEC] client left before the changes were sent; {restored} file(s) restored"
                );
                return Err(e.into());
            }
            refresh_engines(workspace_manager, &workspace, &files).await;
        }
    }

    framed
        .send(WireMessage::RemoteExecResult(RemoteExecResult {
            exit_code,
            duration_ms,
            server_workspace_root: workspace_str,
            timed_out,
            error: exec_err,
            usage,
            platform: Some(prod_code_protocol::platform()),
            diagnostics,
            tests_passed,
            tests_failed,
            tests_skipped,
            test_failures,
            benches,
        }))
        .await?;

    Ok(())
}

/// What `wait4` says about a finished child: its raw wait status and what it and the
/// descendants it waited for used.
fn wait_with_usage(
    pid: i32,
    exited: &std::sync::Mutex<bool>,
) -> Option<(i32, prod_code_protocol::ExecUsage)> {
    // Wait for the exit without reaping, so that until `exited` is set under the lock the pid
    // can only be this child's, running or a zombie: a kill cannot reach a recycled pid.
    loop {
        // SAFETY: an all-zero `siginfo_t` is a valid value for `waitid` to fill in.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // SAFETY: `info` is valid for writes; WNOWAIT leaves the child to be reaped below.
        let got = unsafe {
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOWAIT,
            )
        };
        if got == 0 {
            break;
        }
        if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return None;
        }
    }
    let mut done = exited.lock().unwrap_or_else(|e| e.into_inner());
    *done = true;
    exec_shim::reap_with_usage(pid)
}

/// Kills the process group led by `pid` (the command and everything it spawned), and `pid`
/// itself, unless the child has already exited: then the pid may no longer be its own.
fn kill_exec_group(pid: u32, exited: &std::sync::Mutex<bool>) {
    let done = exited.lock().unwrap_or_else(|e| e.into_inner());
    if *done {
        return;
    }
    let _ = std::process::Command::new("kill")
        .args(["-9", "--", &format!("-{pid}")])
        .status();
    let _ = std::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status();
}

/// Apply batch file synchronization to server workspace storage.
pub async fn handle_client(
    stream: impl Into<AnyStream>,
    addr: impl std::fmt::Display,
    state: Arc<ServerState>,
) -> Result<()> {
    let mut framed = Framed::new(stream.into(), ProdCodeCodec::new());

    // A gateway with a token serves nothing, not even its status, to a connection that does
    // not open with it (#402).
    if let Some(expected) = state.auth_token.as_deref() {
        let first = tokio::time::timeout(AUTH_WAIT, framed.next())
            .await
            .ok()
            .flatten();
        match &first {
            Some(Ok(WireMessage::HttpProbe { method, path })) => {
                tracing::info!(%addr, %method, %path, "HTTP probe received on token-protected gateway port");
                let (status, payload) = if path == "/health" || path == "/healthz" {
                    (
                        200,
                        serde_json::json!({ "status": "ok", "service": "prod-code-gateway" }),
                    )
                } else {
                    (
                        426,
                        serde_json::json!({
                            "error": "protocol_mismatch",
                            "message": "Port 9400 serves prod-code remote code intelligence using a binary framing protocol (or TLS), not general HTTP."
                        }),
                    )
                };
                let _ = framed
                    .send(WireMessage::HttpResponse {
                        status,
                        content_type: "application/json".to_string(),
                        body: serde_json::to_string_pretty(&payload).unwrap_or_default(),
                    })
                    .await;
                return Ok(());
            }
            Some(Ok(WireMessage::Auth(token))) if token.matches(expected) => {}
            _ => {
                let presented = match &first {
                    Some(Ok(WireMessage::Auth(token))) => Some(token),
                    _ => None,
                };
                tracing::warn!(
                    %addr,
                    presented = presented.is_some(),
                    "🔒 [AUTH] closed a connection without the cluster's token"
                );
                let _ = framed
                    .send(WireMessage::Disconnect {
                        reason: AUTH_REFUSED.to_string(),
                    })
                    .await;
                return Ok(());
            }
        }
    }

    while let Some(msg_res) = framed.next().await {
        let msg = msg_res?;
        match msg {
            // A token sent to a gateway that requires none, or sent twice, changes nothing.
            WireMessage::Auth(_) => {}
            WireMessage::HttpProbe { method, path } => {
                tracing::info!(%addr, %method, %path, "HTTP probe/request received on gateway port");
                let (status, payload) = if path == "/health" || path == "/healthz" {
                    (
                        200,
                        serde_json::json!({ "status": "ok", "service": "prod-code-gateway" }),
                    )
                } else {
                    (
                        426,
                        serde_json::json!({
                            "error": "protocol_mismatch",
                            "message": "Port 9400 serves prod-code remote code intelligence using a binary framing protocol (or TLS), not general HTTP. Connect using the prod-code CLI or MCP server. For health checks, GET /health is supported."
                        }),
                    )
                };
                let _ = framed
                    .send(WireMessage::HttpResponse {
                        status,
                        content_type: "application/json".to_string(),
                        body: serde_json::to_string_pretty(&payload).unwrap_or_default(),
                    })
                    .await;
                return Ok(());
            }
            WireMessage::StatusRequest => {
                let status = state.status().await;
                framed.send(WireMessage::StatusResponse(status)).await?;
            }
            WireMessage::Gossip(gossip) => {
                state.absorb_gossip(gossip).await;
                let own = state.own_gossip().await;
                framed.send(WireMessage::Gossip(own)).await?;
            }
            WireMessage::ClusterRequest => {
                let view = state.cluster_view().await;
                framed.send(WireMessage::ClusterResponse(view)).await?;
            }
            WireMessage::PlaceRequest(req) => {
                let resp = state.place(&req).await;
                framed.send(WireMessage::PlaceResponse(resp)).await?;
            }
            WireMessage::MetricsRequest(req) => {
                let node = state.advertise.read().await.clone();
                let resp = state.metrics.summary(&node, req.since_secs);
                framed.send(WireMessage::MetricsResponse(resp)).await?;
            }
            WireMessage::SyncRequest(req) => {
                let workspace = workspace::server_workspace_path(
                    &state.storage_root,
                    &req.client_workspace_root,
                    req.base_workspace_name.as_deref(),
                );
                let touched: Vec<String> =
                    req.files.iter().map(|f| f.relative_path.clone()).collect();
                let resp = apply_sync_with_metrics(
                    &state.storage_root,
                    &state.workspace_manager,
                    Some(&state.metrics),
                    req,
                )
                .await;
                // The files an agent is editing are the ones it validates next: warm them now
                // (#233).
                let synced_rust = priming::synced_rust_files(&workspace, &touched);
                if !synced_rust.is_empty()
                    && let Some(loaded) = state.workspace_manager.get_loaded(&workspace).await
                    && loaded.rust_engine.is_some()
                {
                    // Validation runs on its own engine: that is the one to warm. Loading it
                    // warms the newest files, these among them.
                    let workspace = workspace.clone();
                    let admission = Arc::clone(state.workspace_manager.admission());
                    tokio::spawn(async move {
                        if let Ok(view) = loaded.validation_view(&admission).await
                            && let Some(engine) = view.rust_engine.clone()
                        {
                            priming::warm_in_background(engine, workspace, synced_rust);
                        }
                    });
                }
                // The search index is kept current by what the sync wrote, so a query never
                // has to walk the tree.
                state.search_indexes.invalidate(&workspace, touched);
                framed.send(WireMessage::SyncResponse(resp)).await?;
            }
            WireMessage::SyncProbeRequest(req) => {
                let resp =
                    apply_sync_probe(&state.storage_root, &state.workspace_manager, req).await;
                framed.send(WireMessage::SyncProbeResponse(resp)).await?;
            }
            WireMessage::ExecRequest(req) => {
                run_exec_with_ram(
                    &state.storage_root,
                    &state.metrics,
                    &state.workspace_manager,
                    &mut framed,
                    req,
                    state.build_cache_ram,
                    state.build_cache_dir.as_deref(),
                )
                .await?;
            }
            WireMessage::RemoteExecRequest(req) => {
                run_remote_exec_with_ram(
                    &state.storage_root,
                    &state.metrics,
                    &state.workspace_manager,
                    &mut framed,
                    req,
                    state.build_cache_ram,
                    state.build_cache_dir.as_deref(),
                )
                .await?;
            }
            WireMessage::ShadowRunRequest(req) => {
                shadow::run_shadow(&state, &mut framed, req).await?;
            }
            WireMessage::SearchRequest(req) => {
                let resp = {
                    let state = Arc::clone(&state);
                    tokio::task::spawn_blocking(move || {
                        search::run_search(&state.search_indexes, &state.storage_root, &req)
                    })
                    .await?
                };
                framed.send(WireMessage::SearchResponse(resp)).await?;
            }
            WireMessage::ReadFileRequest(req) => {
                let resp = read_server_file(&state.storage_root, &req);
                framed.send(WireMessage::ReadFileResponse(resp)).await?;
            }
            WireMessage::Ping => {
                framed.send(WireMessage::Pong).await?;
            }
            WireMessage::HandshakeRequest(req) => {
                let protocol_version = match negotiate_protocol_version(&req) {
                    Ok(version) => version,
                    Err(err) => {
                        let reason = format!("gateway refused protocol negotiation: {err}");
                        tracing::warn!(reason, "refusing incompatible handshake");
                        framed.send(WireMessage::Disconnect { reason }).await?;
                        return Ok(());
                    }
                };
                let session_id = state.next_session_id.fetch_add(1, Ordering::Relaxed);
                let _active_session = ActiveSession::start(&state.active_sessions);
                let session_capabilities = prod_code_protocol::negotiate_capabilities(
                    req.capabilities.as_ref(),
                    &prod_code_protocol::default_server_capabilities(),
                );

                let client_root_path = PathBuf::from(&req.client_workspace_root);
                let server_workspace = workspace::resolve_server_workspace(
                    &state.storage_root,
                    &req.client_workspace_root,
                    req.base_workspace_name.as_deref(),
                );
                let server_workspace_str = server_workspace.to_string_lossy().to_string();
                workspace::touch_last_used(&server_workspace);

                // A nested project of another language (engine_subpath) gets its own engine
                // rooted there; sync and path translation stay on the checkout root.
                let engine_root = match req.engine_subpath.as_deref() {
                    Some(sub)
                        if !sub.is_empty()
                            && !sub.starts_with('/')
                            && !sub.split('/').any(|c| c == "..")
                            && server_workspace.join(sub).is_dir() =>
                    {
                        server_workspace.join(sub)
                    }
                    Some(sub) if !sub.is_empty() => {
                        tracing::warn!(subpath = sub, "engine_subpath ignored (missing or unsafe)");
                        server_workspace.clone()
                    }
                    _ => server_workspace.clone(),
                };

                let engine_kind =
                    detect::resolve_engine(&engine_root, req.preferred_engine.as_deref());
                let engine = engine_kind.as_str();
                let supports_redirects = req
                    .capabilities
                    .as_ref()
                    .is_some_and(|capabilities| capabilities.redirects);
                if !state.serves_engine(engine) {
                    if supports_redirects && req.redirect_count < 2 {
                        let view = state.cluster_view().await;
                        if let Some(target) = view
                            .nodes
                            .iter()
                            .find(|n| n.alive && cluster_supports_engine(&n.status, engine))
                        {
                            tracing::info!(
                                engine,
                                target = %target.addr,
                                "redirecting client to cluster node serving engine"
                            );
                            let _ = framed
                                .send(WireMessage::Redirect {
                                    target_addr: target.addr.clone(),
                                    reason: Some(format!(
                                        "engine {engine} is served by {}",
                                        target.addr
                                    )),
                                })
                                .await;
                            return Ok(());
                        }
                    }
                    let reason = format!(
                        "engine {engine} is not served by this node (--engines {}); pick a node that lists it",
                        state.engine_allowlist.join(",")
                    );
                    tracing::warn!(
                        client_root = %req.client_workspace_root,
                        engine,
                        "refusing handshake: engine not served here"
                    );
                    framed.send(WireMessage::Disconnect { reason }).await?;
                    return Ok(());
                }

                // Roadmap 5.1: If this workspace is not already loaded locally, but another live cluster
                // node has it loaded warm, transparently redirect the client there.
                let is_loaded_locally = state
                    .workspace_manager
                    .get_loaded(&server_workspace)
                    .await
                    .is_some();
                if supports_redirects && req.redirect_count < 3 {
                    let view = state.cluster_view().await;
                    let own_addr = state.advertise.read().await.clone();
                    let ws_name = req
                        .base_workspace_name
                        .as_deref()
                        .unwrap_or(&req.client_workspace_root);
                    if !is_loaded_locally && req.redirect_count == 0 {
                        if let Some(holder) = view.nodes.iter().find(|n| {
                            n.alive
                                && n.addr != own_addr
                                && cluster_supports_engine(&n.status, engine)
                                && n.workspaces.iter().any(|w| w.name == ws_name)
                        }) {
                            tracing::info!(
                                workspace = ws_name,
                                target = %holder.addr,
                                "transparently redirecting client to node with warm workspace engine"
                            );
                            let _ = framed
                                .send(WireMessage::Redirect {
                                    target_addr: holder.addr.clone(),
                                    reason: Some(format!(
                                        "workspace {ws_name} is already warm on {}",
                                        holder.addr
                                    )),
                                })
                                .await;
                            return Ok(());
                        }
                    }

                    // Roadmap 5.3: Dynamic workload rebalancing and resource pressure load shedding.
                    // If this gateway is under resource pressure or congested, and another live node is roomy,
                    // redirect this workspace connection to the quietest roomiest node!
                    // If already loaded locally, require severe congestion or hard pressure to avoid unnecessary churn.
                    let current_status = state.status().await;
                    let under_pressure = current_status.host.pressure().is_some();
                    let congested = if is_loaded_locally {
                        under_pressure || current_status.congestion_score() >= 1.5
                    } else {
                        under_pressure || current_status.congestion_score() >= 1.2
                    };
                    if congested {
                        let own_score = current_status.congestion_score();
                        if let Some(roomy) = view
                            .nodes
                            .iter()
                            .filter(|n| {
                                n.alive
                                    && n.addr != own_addr
                                    && cluster_supports_engine(&n.status, engine)
                                    && n.status.host.pressure().is_none()
                                    && n.status.congestion_score() < own_score * 0.6
                            })
                            .min_by(|a, b| {
                                a.status
                                    .congestion_score()
                                    .partial_cmp(&b.status.congestion_score())
                                    .unwrap_or(std::cmp::Ordering::Equal)
                            })
                        {
                            tracing::info!(
                                workspace = ws_name,
                                target = %roomy.addr,
                                loaded = is_loaded_locally,
                                "transparently redirecting client from congested gateway to roomier node"
                            );
                            let _ = framed
                                .send(WireMessage::Redirect {
                                    target_addr: roomy.addr.clone(),
                                    reason: Some(format!(
                                        "node {own_addr} is congested (score {:.2}); redirected to roomier node {}",
                                        own_score,
                                        roomy.addr
                                    )),
                                })
                                .await;
                            return Ok(());
                        }
                    }
                }

                let translator =
                    PathTranslator::new(&req.client_workspace_root, &server_workspace_str);

                // An editor gets the language server it would run locally, a process of its own
                // on this node (#332); without one here, the shared engines answer it.
                if req.purpose.as_deref() == Some(prod_code_protocol::PURPOSE_EDITOR)
                    && editor_proxy::enabled()
                    && let Some(command) =
                        editor_proxy::server_command_for_workspace(engine, &server_workspace)
                {
                    framed
                        .send(WireMessage::HandshakeResponse(HandshakeResponse {
                            protocol_version,
                            server_pid: state.server_pid,
                            session_id,
                            server_workspace_root: server_workspace_str.clone(),
                            detected_engine: engine.to_string(),
                            stale_paths: workspace::stale_paths(&server_workspace),
                            engine_age_ms: None,
                            index_gated: false,
                            capabilities: Some(session_capabilities.clone()),
                        }))
                        .await?;
                    let outcome = editor_proxy::run(
                        framed,
                        translator,
                        command,
                        &engine_root,
                        &state.workspace_manager.editor_servers,
                        session_id,
                    )
                    .await;
                    return outcome;
                }

                // Attach to shared workspace using leader-follower coalescing. A load refused
                // for capacity (#433), or failed, is told to the client, which says why.
                let shared_ws = match state
                    .workspace_manager
                    .get_or_load(&engine_root, engine)
                    .await
                {
                    Ok(shared_ws) => shared_ws,
                    Err(err) => {
                        let reason = format!("{err:#}");
                        tracing::warn!(
                            client_root = %req.client_workspace_root,
                            engine,
                            reason,
                            "refusing handshake: the engine could not be loaded"
                        );
                        framed.send(WireMessage::Disconnect { reason }).await?;
                        return Ok(());
                    }
                };
                let engine_age_ms = shared_ws.loaded_at.elapsed().as_millis() as u64;
                // The in-process Rust engine answers from a complete analysis once loaded; gopls
                // and the servers whose readiness is known are waited for (#391).
                let index_gated = shared_ws.rust_engine.is_some()
                    || shared_ws.go_engine.is_some()
                    || shared_ws
                        .generic_engine
                        .as_ref()
                        .is_some_and(|engine| engine.readiness_known());

                let validation =
                    req.purpose.as_deref() == Some(prod_code_protocol::PURPOSE_VALIDATION);
                let generic_validation_session = if validation && shared_ws.generic_engine.is_some()
                {
                    Some(Arc::clone(&shared_ws.generic_validation_session))
                } else {
                    None
                };
                let mut session_view = state
                    .workspace_manager
                    .register_session_view(session_id, client_root_path.clone(), shared_ws)
                    .await;
                // A session that only validates proposed texts runs on the workspace's second
                // engine, so its overlays never invalidate the main one (#73).
                let _generic_validation_session = if let Some(serial) = generic_validation_session {
                    Some(serial.lock_owned().await)
                } else {
                    None
                };
                if validation {
                    let validation_view = session_view
                        .accounted
                        .validation_view(state.workspace_manager.admission())
                        .await;
                    match validation_view {
                        Ok(view) => session_view.workspace = view,
                        Err(err) => {
                            let is_capacity = err
                                .downcast_ref::<crate::admission::CapacityRefused>()
                                .is_some()
                                || err.root_cause().is::<crate::admission::CapacityRefused>()
                                || err.to_string().contains("capacity:");

                            if is_capacity && req.redirect_count < 2 {
                                let view = state.cluster_view().await;
                                let own_addr = state.advertise.read().await.clone();
                                let ws_name = req
                                    .base_workspace_name
                                    .as_deref()
                                    .unwrap_or(&req.client_workspace_root);
                                let required_headroom =
                                    state.workspace_manager.admission().reserve_for(engine);
                                let target = view
                                    .nodes
                                    .iter()
                                    .filter(|n| {
                                        n.alive
                                            && !n.addr.is_empty()
                                            && n.addr != own_addr
                                            && n.addr != view.this_node
                                            && cluster_supports_engine(&n.status, engine)
                                            && n.status.host.pressure().is_none()
                                            && n.status.host.memory_available_bytes.is_none_or(
                                                |available| available >= required_headroom,
                                            )
                                            && (req.redirect_count == 0
                                                || !n.workspaces.iter().any(|w| w.name == ws_name))
                                    })
                                    .max_by_key(|n| {
                                        n.status.host.memory_available_bytes.unwrap_or(0)
                                    });

                                if let Some(target) = target {
                                    tracing::info!(
                                        session_id,
                                        engine,
                                        target = %target.addr,
                                        "redirecting validation session under gateway memory pressure"
                                    );
                                    state
                                        .workspace_manager
                                        .unregister_session_view(session_view)
                                        .await;
                                    let _ = framed
                                        .send(WireMessage::Redirect {
                                            target_addr: target.addr.clone(),
                                            reason: Some(format!(
                                                "gateway memory pressure; validating {engine} on {}",
                                                target.addr
                                            )),
                                        })
                                        .await;
                                    return Ok(());
                                }
                            }

                            let reason = format!("private validation engine unavailable: {err:#}");
                            tracing::warn!(
                                session_id,
                                engine,
                                reason,
                                "refusing validation handshake"
                            );
                            state
                                .workspace_manager
                                .unregister_session_view(session_view)
                                .await;
                            framed.send(WireMessage::Disconnect { reason }).await?;
                            return Ok(());
                        }
                    }
                }

                tracing::info!(
                    session_id,
                    client_pid = req.client_pid,
                    client_root = %req.client_workspace_root,
                    server_root = %server_workspace_str,
                    engine_root = %engine_root.display(),
                    engine,
                    is_single_owner = session_view.is_single_owner(),
                    "Client session established (Direct-Edit fast path active: {})",
                    session_view.is_single_owner()
                );

                framed
                    .send(WireMessage::HandshakeResponse(HandshakeResponse {
                        protocol_version,
                        server_pid: state.server_pid,
                        session_id,
                        server_workspace_root: server_workspace_str,
                        detected_engine: engine.to_string(),
                        stale_paths: workspace::stale_paths(&server_workspace),
                        engine_age_ms: Some(engine_age_ms),
                        index_gated,
                        capabilities: Some(session_capabilities),
                    }))
                    .await?;

                // Session loop for streaming LSP and control messages
                let meta = Arc::new(SessionMeta {
                    session_id,
                    client_name: req.client_name.clone(),
                    agent: req
                        .client_agent
                        .clone()
                        .unwrap_or_else(|| "unknown".to_string()),
                    host: req
                        .client_host
                        .clone()
                        .unwrap_or_else(|| "unknown".to_string()),
                    client_addr: addr.to_string(),
                    workspace: engine_root
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    engine: engine.to_string(),
                    engine_root: engine_root.clone(),
                    storage_root: state.storage_root.clone(),
                    metrics: Arc::clone(&state.metrics),
                    editor: req.purpose.as_deref() == Some(prod_code_protocol::PURPOSE_EDITOR),
                    edits: Arc::default(),
                });
                let session_res = run_session_loop(framed, &translator, &session_view, meta).await;

                state
                    .workspace_manager
                    .unregister_session_view(session_view)
                    .await;

                tracing::debug!(session_id, "Client session retired: {:?}", session_res);
                return session_res;
            }
            WireMessage::Disconnect { reason } => {
                tracing::debug!(%addr, reason, "Client disconnected cleanly");
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
    framed: Framed<AnyStream, ProdCodeCodec>,
    translator: &PathTranslator,
    view: &SessionView,
    meta: Arc<SessionMeta>,
) -> Result<()> {
    let (mut socket_tx, mut socket_rx) = framed.split();
    // rapidfire MPSC: every engine task sends, one writer drains in batches and flushes the
    // socket once per batch.
    let (raw_out_tx, mut out_rx) =
        rapidfire::mpsc::bounded::<SharedOutputFrame>(SHARED_OUTPUT_CAPACITY);
    let out_tx = SharedOutputSender::new(raw_out_tx, SHARED_OUTPUT_WRITE_BUDGET);

    // Requests in flight, keyed by JSON-RPC id, so every answer — whichever engine produced
    // it — becomes one metrics event with its duration.
    let pending: Arc<tokio::sync::Mutex<std::collections::HashMap<String, PendingRequest>>> =
        Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
    let pending_writer = Arc::clone(&pending);
    let meta_writer = Arc::clone(&meta);
    let writer_output = out_tx.clone();
    let writer_handle = tokio::spawn(async move {
        let _lifetime = SharedWriterLifetime::start(writer_output);
        let mut batch: Vec<SharedOutputFrame> = Vec::with_capacity(SHARED_OUTPUT_BATCH);
        while out_rx
            .recv_many(&mut batch, SHARED_OUTPUT_BATCH)
            .await
            .is_ok()
        {
            let mut flush_deadline = None;
            for frame in batch.drain(..) {
                if tokio::time::Instant::now() >= frame.deadline {
                    anyhow::bail!("shared output frame expired while queued");
                }
                if let WireMessage::LspPayload(ref raw) = frame.message
                    && let Ok(val) = serde_json::from_str::<serde_json::Value>(raw)
                    && let Some(id) = val.get("id").filter(|i| !i.is_null())
                    && val.get("method").is_none()
                {
                    let key = id.to_string();
                    if let Some(req) = pending_writer.lock().await.remove(&key) {
                        let mut ev = metrics::Event::blank("lsp");
                        ev.session_id = meta_writer.session_id;
                        ev.client_name = meta_writer.client_name.clone();
                        ev.agent = meta_writer.agent.clone();
                        ev.host = meta_writer.host.clone();
                        ev.client_addr = meta_writer.client_addr.clone();
                        ev.workspace = meta_writer.workspace.clone();
                        ev.engine = meta_writer.engine.clone();
                        ev.method = req.method;
                        ev.file = req.file;
                        ev.line = req.line;
                        ev.col = req.col;
                        ev.duration_ms = req.start.elapsed().as_millis() as u64;
                        ev.ok = val.get("error").is_none();
                        ev.items = val
                            .get("result")
                            .map(|r| match r {
                                serde_json::Value::Array(a) => a.len() as u64,
                                serde_json::Value::Null => 0,
                                _ => 1,
                            })
                            .unwrap_or(0);
                        meta_writer.metrics.record(ev);
                    }
                }
                tokio::time::timeout_at(frame.deadline, socket_tx.feed(frame.message))
                    .await
                    .map_err(|_| anyhow::anyhow!("shared socket feed deadline elapsed"))??;
                flush_deadline = Some(match flush_deadline {
                    Some(current) => std::cmp::min(current, frame.deadline),
                    None => frame.deadline,
                });
            }
            if let Some(deadline) = flush_deadline {
                tokio::time::timeout_at(deadline, socket_tx.flush())
                    .await
                    .map_err(|_| anyhow::anyhow!("shared socket flush deadline elapsed"))??;
            }
        }
        Ok(())
    });
    // Install abort ownership before the session can reach another await. Cancellation of the
    // handler must cancel this exact writer, whose lifetime guard closes every producer queue.
    let mut writer = OwnedJoin::new(writer_handle);

    // gopls and the supervised servers answer their own requests (`window/workDoneProgress/create`,
    // `workspace/configuration`) in the engine; passed on, one carried the id of a client's
    // question and was taken for its answer (#391). Only their notifications go to the client.
    let engine_answers_requests =
        view.workspace.go_engine.is_some() || view.workspace.generic_engine.is_some();
    let mut backend_rx = if let Some(ref go) = view.workspace.go_engine {
        Some(go.subscribe())
    } else if let Some(ref generic_eng) = view.workspace.generic_engine {
        Some(generic_eng.subscribe())
    } else {
        view.workspace.backend.as_ref().map(|b| b.subscribe())
    };

    let mut rebalance_rx = view.accounted.subscribe_rebalance();
    let mut writer_finished = false;
    let mut session_result = Ok(());
    loop {
        tokio::select! {
            writer_result = writer.task_mut() => {
                writer.clear_finished();
                writer_finished = true;
                session_result = flatten_writer_result(writer_result);
                break;
            }
            client_msg_res = socket_rx.next() => {
                match on_client_message(client_msg_res, &out_tx, translator, view, &meta, &pending).await {
                    Flow::Next => continue,
                    Flow::Stop => break,
                }
            }

            rebalance_msg = rebalance_rx.recv() => {
                match rebalance_msg {
                    Ok((target_addr, reason)) => {
                        tracing::info!(
                            session_id = meta.session_id,
                            target = %target_addr,
                            ?reason,
                            "Session rebalanced: sending Redirect frame to active client"
                        );
                        let _ = out_tx.send(WireMessage::Redirect { target_addr, reason }).await;
                        break;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {}
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
                        if (engine_answers_requests && is_server_request(&server_lsp))
                            || (!engine_answers_requests && fallback_answers_request(&server_lsp))
                        {
                            continue;
                        }
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
    out_tx.close();
    if !writer_finished {
        let teardown_deadline = tokio::time::Instant::now() + SHARED_OUTPUT_TEARDOWN_BUDGET;
        match tokio::time::timeout_at(teardown_deadline, writer.task_mut()).await {
            Ok(writer_result) => {
                writer.clear_finished();
                let writer_result = flatten_writer_result(writer_result);
                if session_result.is_ok() {
                    session_result = writer_result;
                }
            }
            Err(_) => {
                writer.abort();
                // Once aborted, await the exact task so no writer is detached. This is cleanup
                // after the single teardown deadline, not a second drain budget.
                let writer_result = writer.task_mut().await;
                writer.clear_finished();
                if let Err(error) = writer_result
                    && !error.is_cancelled()
                {
                    tracing::warn!(%error, "shared output writer failed while being aborted");
                }
                if session_result.is_ok() {
                    session_result = Err(anyhow::anyhow!(
                        "shared output writer exceeded teardown budget"
                    ));
                }
            }
        }
    }
    session_result
}

const SHARED_OUTPUT_CAPACITY: usize = 64;
const SHARED_OUTPUT_BATCH: usize = 64;
const SHARED_OUTPUT_WRITE_BUDGET: Duration = Duration::from_secs(2);
const SHARED_OUTPUT_TEARDOWN_BUDGET: Duration = Duration::from_secs(3);

#[doc(hidden)]
pub static ACTIVE_SHARED_OUTPUT_WRITERS: AtomicUsize = AtomicUsize::new(0);

struct SharedOutputFrame {
    message: WireMessage,
    deadline: tokio::time::Instant,
}

#[derive(Clone)]
struct SharedOutputSender {
    inner: rapidfire::mpsc::Sender<SharedOutputFrame>,
    write_budget: Duration,
}

#[derive(Debug)]
enum SharedOutputSendError {
    Closed,
    Deadline,
}

impl SharedOutputSender {
    fn new(inner: rapidfire::mpsc::Sender<SharedOutputFrame>, write_budget: Duration) -> Self {
        Self {
            inner,
            write_budget,
        }
    }

    async fn send(&self, message: WireMessage) -> std::result::Result<(), SharedOutputSendError> {
        let deadline = tokio::time::Instant::now() + self.write_budget;
        let frame = SharedOutputFrame { message, deadline };
        match tokio::time::timeout_at(deadline, self.inner.send(frame)).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) => Err(SharedOutputSendError::Closed),
            Err(_) => {
                // One expired producer expires the generation. This wakes every queue waiter and
                // prevents later notifications from repeatedly extending a dead client's life.
                self.close();
                Err(SharedOutputSendError::Deadline)
            }
        }
    }

    fn close(&self) {
        self.inner.close();
    }
}

struct SharedWriterLifetime {
    output: SharedOutputSender,
}

impl SharedWriterLifetime {
    fn start(output: SharedOutputSender) -> Self {
        ACTIVE_SHARED_OUTPUT_WRITERS.fetch_add(1, Ordering::Relaxed);
        Self { output }
    }
}

impl Drop for SharedWriterLifetime {
    fn drop(&mut self) {
        self.output.close();
        ACTIVE_SHARED_OUTPUT_WRITERS.fetch_sub(1, Ordering::Relaxed);
    }
}

struct OwnedJoin<T> {
    task: Option<tokio::task::JoinHandle<T>>,
}

impl<T> OwnedJoin<T> {
    fn new(task: tokio::task::JoinHandle<T>) -> Self {
        Self { task: Some(task) }
    }

    fn task_mut(&mut self) -> &mut tokio::task::JoinHandle<T> {
        self.task.as_mut().expect("owned task is live")
    }

    fn abort(&self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }

    fn clear_finished(&mut self) {
        let task = self.task.take().expect("owned task is live");
        debug_assert!(task.is_finished());
    }
}

impl<T> Drop for OwnedJoin<T> {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

fn flatten_writer_result(
    result: std::result::Result<Result<()>, tokio::task::JoinError>,
) -> Result<()> {
    result.map_err(|error| anyhow::anyhow!("shared output writer task failed: {error}"))?
}

/// The capabilities an editor's `initialize` is answered with (#310).
///
/// A language server on the node advertises its own, except for document sync: the gateway
/// hands every change on as the document's full text, so the editor is asked for exactly that
/// whatever the server would take. The in-memory Rust engine advertises what it answers, with
/// the trigger characters rust-analyzer's own server uses; a workspace with neither only takes
/// documents.
fn editor_capabilities(server: Option<serde_json::Value>, rust: bool) -> serde_json::Value {
    let mut caps = match server {
        Some(caps) if caps.is_object() => caps,
        _ if rust => serde_json::json!({
            "hoverProvider": true,
            "definitionProvider": true,
            "referencesProvider": true,
            "implementationProvider": true,
            "documentSymbolProvider": true,
            "workspaceSymbolProvider": true,
            "renameProvider": true,
            "callHierarchyProvider": true,
            "completionProvider": {
                "triggerCharacters": [":", ".", "'", "("],
                "resolveProvider": true
            },
            "signatureHelpProvider": { "triggerCharacters": ["(", ",", "<"] },
            "inlayHintProvider": true,
            "documentHighlightProvider": true,
            "codeActionProvider": { "resolveProvider": true },
            "documentFormattingProvider": true
        }),
        _ => serde_json::json!({}),
    };
    let save = caps.pointer("/textDocumentSync/save").cloned();
    caps["textDocumentSync"] = serde_json::json!({ "openClose": true, "change": 1 });
    if let Some(save) = save {
        caps["textDocumentSync"]["save"] = save;
    }
    caps
}

/// What the session loop does once a client message has been handled.
enum Flow {
    /// Wait for the next message.
    Next,
    /// The client is gone or asked to disconnect: end the session.
    Stop,
}

/// Decode an LSP's zero-based position into the one-based coordinates used by the Rust engine.
///
/// LSP positions must be non-negative JSON integers. The engine's one-based API also means
/// that `u32::MAX` cannot be represented, so reject it rather than truncating or overflowing.
fn one_based_position(position: Option<&serde_json::Value>) -> Result<(u32, u32), &'static str> {
    let position = position.ok_or("position is required")?;
    let coordinate = |name| {
        position
            .get(name)
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .and_then(|value| value.checked_add(1))
            .ok_or("position coordinates must be non-negative integers below 4294967295")
    };
    Ok((coordinate("line")?, coordinate("character")?))
}

/// Keep request metrics bounded even when a forwarded request has an arbitrary JSON shape.
fn metric_position(position: Option<&serde_json::Value>) -> (u32, u32) {
    one_based_position(position).unwrap_or((1, 1))
}

/// Validate only the Rust methods that consume LSP positions locally.
fn native_position_params(
    method: Option<&str>,
    params: Option<&serde_json::Value>,
) -> Result<(), String> {
    let params = params.unwrap_or(&serde_json::Value::Null);
    match method {
        Some(
            "textDocument/hover"
            | "textDocument/definition"
            | "textDocument/references"
            | "textDocument/implementation"
            | "textDocument/prepareCallHierarchy"
            | "prodCode/safeDelete"
            | "textDocument/rename",
        ) => one_based_position(params.get("position"))
            .map(|_| ())
            .map_err(str::to_owned),
        Some("callHierarchy/incomingCalls" | "callHierarchy/outgoingCalls") => one_based_position(
            params
                .get("item")
                .and_then(|item| item.get("selectionRange"))
                .and_then(|range| range.get("start")),
        )
        .map(|_| ())
        .map_err(|reason| format!("item.selectionRange.start: {reason}")),
        Some("prodCode/structuralReplace") => params
            .get("position")
            .map(|position| {
                one_based_position(Some(position))
                    .map(|_| ())
                    .map_err(str::to_owned)
            })
            .unwrap_or(Ok(())),
        Some("prodCode/assists" | "prodCode/applyAssist") => {
            one_based_position(params.pointer("/range/start"))
                .map_err(|reason| format!("range.start: {reason}"))?;
            if let Some(end) = params.pointer("/range/end") {
                one_based_position(Some(end)).map_err(|reason| format!("range.end: {reason}"))?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

async fn send_invalid_params(
    out_tx: &SharedOutputSender,
    translator: &PathTranslator,
    id: &serde_json::Value,
    method: &str,
    reason: &str,
) {
    let response = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": -32602,
            "message": format!("invalid params for {method}: {reason}"),
        }
    });
    let client_response = translator.translate_lsp_to_client(&response.to_string());
    let _ = out_tx.send(WireMessage::LspPayload(client_response)).await;
}

/// One message from the client: an LSP payload answered by the in-memory engine or forwarded
/// to the backend, a sync, a status request or a disconnect.
///
/// It lived inside the session loop's `tokio::select!`, where it was 1,400 lines of macro
/// input: rust-analyzer offers no refactoring inside a macro call, and every validation of the
/// file inferred it as one body (#86). Out here it is ordinary code.
async fn on_client_message(
    client_msg_res: Option<std::result::Result<WireMessage, std::io::Error>>,
    out_tx: &SharedOutputSender,
    translator: &PathTranslator,
    view: &SessionView,
    meta: &Arc<SessionMeta>,
    pending: &Arc<tokio::sync::Mutex<std::collections::HashMap<String, PendingRequest>>>,
) -> Flow {
    match client_msg_res {
        Some(Ok(WireMessage::Ping)) => {
            let _ = out_tx.send(WireMessage::Pong).await;
        }
        Some(Ok(WireMessage::LspPayload(raw_client_lsp))) => {
            let server_lsp = translator.translate_lsp_to_server(&raw_client_lsp);
            tracing::debug!(
                payload_len = server_lsp.len(),
                single_owner = view.is_single_owner(),
                "Processing incoming LSP message"
            );

            // Inspect LSP message structure
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&server_lsp) {
                let method = val.get("method").and_then(|m| m.as_str());
                let id = val.get("id").cloned();
                if view.workspace.rust_engine.is_some()
                    && let Err(reason) = native_position_params(method, val.get("params"))
                {
                    if let (Some(method), Some(id)) =
                        (method, id.as_ref().filter(|id| !id.is_null()))
                    {
                        send_invalid_params(out_tx, translator, id, method, &reason).await;
                    }
                    return Flow::Next;
                }
                if let (Some(m), Some(id_val)) = (method, &id)
                    && !id_val.is_null()
                    && m != "initialize"
                {
                    let params = val.get("params");
                    let uri = params
                        .and_then(|p| {
                            p.get("textDocument")
                                .and_then(|t| t.get("uri"))
                                .or_else(|| p.get("item").and_then(|i| i.get("uri")))
                        })
                        .and_then(|u| u.as_str())
                        .unwrap_or("");
                    let path = uri_or_path(uri);
                    let file = path
                        .strip_prefix(&meta.engine_root)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .into_owned();
                    let pos = params.and_then(|p| {
                        p.get("position")
                            .or_else(|| p.get("range").and_then(|r| r.get("start")))
                            .or_else(|| {
                                p.get("item")
                                    .and_then(|item| item.get("selectionRange"))
                                    .and_then(|range| range.get("start"))
                            })
                    });
                    let (line, col) = metric_position(pos);
                    pending.lock().await.insert(
                        id_val.to_string(),
                        PendingRequest {
                            method: m.to_string(),
                            file,
                            line,
                            col,
                            start: Instant::now(),
                        },
                    );
                }

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
                    let caps = editor_capabilities(caps, view.workspace.rust_engine.is_some());
                    let init_resp = serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": req_id,
                        "result": {
                            "capabilities": caps,
                            "serverInfo": {
                                "name": "prod-code",
                                "version": env!("CARGO_PKG_VERSION")
                            }
                        }
                    });
                    let client_resp = translator.translate_lsp_to_client(&init_resp.to_string());
                    let _ = out_tx.send(WireMessage::LspPayload(client_resp)).await;
                    return Flow::Next;
                }

                // 2. Intercept "initialized": backend already initialized, consume without forwarding
                if method == Some("initialized") {
                    return Flow::Next;
                }

                // 3. Intercept "shutdown": reply cleanly
                if method == Some("shutdown") {
                    let req_id = id.unwrap_or(serde_json::json!(1));
                    let shutdown_resp = serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": req_id,
                        "result": null
                    });
                    let _ = out_tx
                        .send(WireMessage::LspPayload(shutdown_resp.to_string()))
                        .await;
                    return Flow::Next;
                }

                // 4. In-Memory RustEngine multi-core fast path: hover, definition, references, documentSymbol
                if let Some(ref engine_lock) = view.workspace.rust_engine {
                    match method {
                        Some("textDocument/hover") => {
                            if let Some(params) = val.get("params") {
                                lsp_hover(out_tx, translator, view, &id, params, engine_lock);
                                return Flow::Next;
                            }
                        }
                        Some("textDocument/definition") => {
                            if let Some(params) = val.get("params") {
                                lsp_definition(out_tx, translator, view, &id, params, engine_lock);
                                return Flow::Next;
                            }
                        }
                        Some("textDocument/references") => {
                            if let Some(params) = val.get("params") {
                                lsp_references(out_tx, translator, view, &id, params, engine_lock);
                                return Flow::Next;
                            }
                        }
                        Some("textDocument/documentSymbol") => {
                            if let Some(params) = val.get("params") {
                                lsp_document_symbol(
                                    out_tx,
                                    translator,
                                    view,
                                    &id,
                                    params,
                                    engine_lock,
                                );
                                return Flow::Next;
                            }
                        }
                        Some("workspace/symbol") => {
                            if let Some(params) = val.get("params") {
                                lsp_workspace_symbol(
                                    out_tx,
                                    translator,
                                    view,
                                    &id,
                                    params,
                                    engine_lock,
                                );
                                return Flow::Next;
                            }
                        }
                        Some("prodCode/assists") | Some("prodCode/applyAssist") => {
                            if let Some(params) = val.get("params") {
                                lsp_assists(
                                    out_tx,
                                    translator,
                                    view,
                                    method,
                                    &id,
                                    params,
                                    engine_lock,
                                );
                                return Flow::Next;
                            }
                        }
                        Some("prodCode/safeDelete") => {
                            if let Some(params) = val.get("params") {
                                lsp_safe_delete(out_tx, translator, view, &id, params, engine_lock);
                                return Flow::Next;
                            }
                        }
                        Some(
                            hm @ ("textDocument/prepareCallHierarchy"
                            | "callHierarchy/incomingCalls"
                            | "callHierarchy/outgoingCalls"
                            | "textDocument/implementation"
                            | "textDocument/diagnostic"),
                        ) => {
                            if let Some(params) = val.get("params") {
                                lsp_call_hierarchy(
                                    out_tx,
                                    translator,
                                    view,
                                    &id,
                                    hm,
                                    params,
                                    engine_lock,
                                );
                                return Flow::Next;
                            }
                        }
                        Some("textDocument/rename") => {
                            if let Some(params) = val.get("params") {
                                lsp_rename(out_tx, translator, view, &id, params, engine_lock);
                                return Flow::Next;
                            }
                        }
                        Some("prodCode/structuralReplace") => {
                            if let Some(params) = val.get("params") {
                                lsp_structural_replace(
                                    out_tx,
                                    translator,
                                    view,
                                    &id,
                                    params,
                                    engine_lock,
                                );
                                return Flow::Next;
                            }
                        }
                        Some(m) if prod_code_engine_rust::editor::EDITOR_METHODS.contains(&m) => {
                            let params = val.get("params").cloned().unwrap_or_default();
                            lsp_editor_request(
                                out_tx,
                                translator,
                                view,
                                &id,
                                m,
                                params,
                                engine_lock,
                            );
                            return Flow::Next;
                        }
                        Some("textDocument/didOpen") => {
                            if let Some(params) = val.get("params") {
                                let uri = params
                                    .get("textDocument")
                                    .and_then(|td| td.get("uri"))
                                    .and_then(|u| u.as_str())
                                    .unwrap_or("");
                                let file_path = uri_or_path(uri);
                                if let Some(text) = params
                                    .get("textDocument")
                                    .and_then(|td| td.get("text"))
                                    .and_then(|t| t.as_str())
                                {
                                    let edit_start = Instant::now();
                                    let text_len = text.len();
                                    {
                                        let mut engine = engine_lock.lock().await;
                                        if view.is_single_owner() {
                                            if let Err(e) = engine
                                                .apply_file_change(&file_path, text.to_string())
                                            {
                                                tracing::warn!(error = %e, file = %file_path.display(), "direct-edit didOpen file change failed");
                                            }
                                            if let Ok(mut files) =
                                                view.direct_edit_open_files.lock()
                                            {
                                                files.insert(file_path.clone(), text.to_string());
                                            }
                                        } else {
                                            if let Err(e) = engine.set_session_overlay(
                                                view.session_id,
                                                &file_path,
                                                Some(text.to_string()),
                                            ) {
                                                tracing::warn!(error = %e, file = %file_path.display(), "session overlay update failed");
                                            }
                                        }
                                    }
                                    let ms = edit_start.elapsed().as_secs_f64() * 1000.0;
                                    tracing::info!(
                                        session = view.session_id,
                                        file = %file_path.display(),
                                        bytes = text_len,
                                        duration_ms = format!("{:.2}ms", ms),
                                        single_owner = view.is_single_owner(),
                                        "📝 [EDIT] didOpen recorded in Salsa DB"
                                    );
                                    publish_rust_diagnostics(
                                        out_tx,
                                        translator,
                                        view,
                                        meta,
                                        file_path.clone(),
                                        engine_lock,
                                    );
                                }
                            }
                        }
                        Some("textDocument/didChange") => {
                            if let Some(params) = val.get("params") {
                                let uri = params
                                    .get("textDocument")
                                    .and_then(|td| td.get("uri"))
                                    .and_then(|u| u.as_str())
                                    .unwrap_or("");
                                let file_path = uri_or_path(uri);
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
                                        if view.is_single_owner() {
                                            if let Err(e) = engine
                                                .apply_file_change(&file_path, text.to_string())
                                            {
                                                tracing::warn!(error = %e, file = %file_path.display(), "direct-edit didChange file change failed");
                                            }
                                            if let Ok(mut files) =
                                                view.direct_edit_open_files.lock()
                                            {
                                                files.insert(file_path.clone(), text.to_string());
                                            }
                                        } else {
                                            if let Err(e) = engine.set_session_overlay(
                                                view.session_id,
                                                &file_path,
                                                Some(text.to_string()),
                                            ) {
                                                tracing::warn!(error = %e, file = %file_path.display(), "session overlay update failed");
                                            }
                                        }
                                    }
                                    let ms = edit_start.elapsed().as_secs_f64() * 1000.0;
                                    tracing::info!(
                                        session = view.session_id,
                                        file = %file_path.display(),
                                        bytes = text_len,
                                        duration_ms = format!("{:.2}ms", ms),
                                        single_owner = view.is_single_owner(),
                                        "📝 [EDIT] didChange recorded in Salsa DB"
                                    );
                                    publish_rust_diagnostics(
                                        out_tx,
                                        translator,
                                        view,
                                        meta,
                                        file_path.clone(),
                                        engine_lock,
                                    );
                                }
                            }
                        }
                        Some("textDocument/didClose") => {
                            if let Some(params) = val.get("params") {
                                let uri = params
                                    .get("textDocument")
                                    .and_then(|td| td.get("uri"))
                                    .and_then(|u| u.as_str())
                                    .unwrap_or("");
                                let file_path = uri_or_path(uri);
                                let mut engine = engine_lock.lock().await;
                                if view.is_single_owner() {
                                    if let Err(e) = engine.reload_file(&file_path) {
                                        tracing::warn!(error = %e, file = %file_path.display(), "direct-edit didClose reload failed");
                                    }
                                    if let Ok(mut files) = view.direct_edit_open_files.lock() {
                                        files.remove(&file_path);
                                    }
                                } else {
                                    if let Err(e) =
                                        engine.clear_session_overlay(view.session_id, &file_path)
                                    {
                                        tracing::warn!(error = %e, file = %file_path.display(), "session overlay close failed");
                                    }
                                }
                            }
                            return Flow::Next;
                        }
                        // A request the in-memory engine has no answer for is refused rather
                        // than left without a reply, which an editor waits on for good.
                        Some(m) if id.as_ref().is_some_and(|i| !i.is_null()) => {
                            let refused = serde_json::json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "error": { "code": -32601, "message": format!("{m} is not supported by prod-code for Rust") }
                            });
                            let _ = out_tx
                                .send(WireMessage::LspPayload(refused.to_string()))
                                .await;
                            return Flow::Next;
                        }
                        _ => {}
                    }
                }

                // 5. Fallback handling for textDocument/didOpen vs didChange on backend worker
                if let (Some("textDocument/didOpen"), Some(backend)) =
                    (method, &view.workspace.backend)
                {
                    let uri = val
                        .get("params")
                        .and_then(|p| p.get("textDocument"))
                        .and_then(|td| td.get("uri"))
                        .and_then(|u| u.as_str())
                        .unwrap_or("");
                    let is_open = backend.open_files.read().await.contains(uri);
                    if is_open {
                        let text = val
                            .get("params")
                            .and_then(|p| p.get("textDocument"))
                            .and_then(|td| td.get("text"))
                            .and_then(|t| t.as_str())
                            .unwrap_or("");
                        let version = val
                            .get("params")
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
                        return Flow::Next;
                    } else {
                        backend.open_files.write().await.insert(uri.to_string());
                    }
                }

                // 5a'. Code actions on managed language servers: the Rust-style
                // prodCode/assists | applyAssist requests become LSP codeAction.
                if let (Some(pm @ ("prodCode/assists" | "prodCode/applyAssist")), Some(req_id)) =
                    (method, &id)
                    && (view.workspace.go_engine.is_some()
                        || view.workspace.generic_engine.is_some())
                {
                    let params = val.get("params").cloned().unwrap_or(serde_json::json!({}));
                    let go = view.workspace.go_engine.clone();
                    let generic = view.workspace.generic_engine.clone();
                    let out_tx_task = out_tx.clone();
                    let translator_task = translator.clone();
                    let r_id = req_id.clone();
                    let session_id = view.session_id;
                    let method_name = pm.to_string();
                    let start = Instant::now();
                    TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
                    tokio::task::spawn(async move {
                        let engine = match (&go, &generic) {
                            (_, Some(g)) => ManagedLsp::Generic(g),
                            (Some(g), None) => ManagedLsp::Go(g),
                            (None, None) => unreachable!("guarded above"),
                        };
                        let outcome = lsp_code_actions(&engine, &method_name, params).await;
                        let ms = start.elapsed().as_secs_f64() * 1000.0;
                        let resp = match outcome {
                            Ok(result) => {
                                tracing::info!(session = session_id, method = %method_name, duration_ms = format!("{ms:.2}ms"), "✅ [LSP DONE] code actions");
                                serde_json::json!({ "jsonrpc": "2.0", "id": r_id, "result": result })
                            }
                            Err(err) => {
                                tracing::info!(session = session_id, method = %method_name, error = %err, "🚫 [LSP REFUSED] code actions");
                                serde_json::json!({ "jsonrpc": "2.0", "id": r_id, "error": { "code": -32602, "message": err.to_string() } })
                            }
                        };
                        let client_resp =
                            translator_task.translate_lsp_to_client(&resp.to_string());
                        let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
                    });
                    return Flow::Next;
                }

                // 5b. Supervised GoEngine fast path
                if let Some(ref go) = view.workspace.go_engine {
                    match method {
                        Some("textDocument/didOpen") => {
                            let uri = val
                                .get("params")
                                .and_then(|p| p.get("textDocument"))
                                .and_then(|td| td.get("uri"))
                                .and_then(|u| u.as_str())
                                .unwrap_or("");
                            let text = val
                                .get("params")
                                .and_then(|p| p.get("textDocument"))
                                .and_then(|td| td.get("text"))
                                .and_then(|t| t.as_str())
                                .unwrap_or("");
                            let _ = go.did_open(uri, text).await;
                            return Flow::Next;
                        }
                        Some("textDocument/didChange") => {
                            let uri = val
                                .get("params")
                                .and_then(|p| p.get("textDocument"))
                                .and_then(|td| td.get("uri"))
                                .and_then(|u| u.as_str())
                                .unwrap_or("");
                            let version = val
                                .get("params")
                                .and_then(|p| p.get("textDocument"))
                                .and_then(|td| td.get("version"))
                                .and_then(|v| v.as_i64())
                                .unwrap_or(1) as i32;
                            let text = val
                                .get("params")
                                .and_then(|p| p.get("contentChanges"))
                                .and_then(|c| c.as_array())
                                .and_then(|a| a.first())
                                .and_then(|ch| ch.get("text"))
                                .and_then(|t| t.as_str())
                                .unwrap_or("");
                            let _ = go.did_change(uri, text, version).await;
                            return Flow::Next;
                        }
                        Some("textDocument/didClose") => {
                            let uri = val
                                .get("params")
                                .and_then(|p| p.get("textDocument"))
                                .and_then(|td| td.get("uri"))
                                .and_then(|u| u.as_str())
                                .unwrap_or("");
                            let _ = go.did_close(uri).await;
                            return Flow::Next;
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

                            let params =
                                val.get("params").cloned().unwrap_or(serde_json::json!({}));
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
                                        send_busy_note(&mut resp, &out_tx_task).await;
                                        if let Some(ref r_id) = req_id {
                                            resp["id"] = r_id.clone();
                                        }
                                        let client_resp = translator_task
                                            .translate_lsp_to_client(&resp.to_string());
                                        let _ = out_tx_task
                                            .send(WireMessage::LspPayload(client_resp))
                                            .await;
                                    }
                                    Err(err) => {
                                        let err_resp = serde_json::json!({
                                            "jsonrpc": "2.0",
                                            "id": req_id,
                                            "error": { "code": -32603, "message": err.to_string() }
                                        });
                                        let _ = out_tx_task
                                            .send(WireMessage::LspPayload(err_resp.to_string()))
                                            .await;
                                    }
                                }
                            });
                            return Flow::Next;
                        }
                        Some(m) => {
                            let params =
                                val.get("params").cloned().unwrap_or(serde_json::json!({}));
                            let _ = go.send_notification(m, params).await;
                            return Flow::Next;
                        }
                        None => {}
                    }
                }

                // 5c. Supervised GenericLspEngine fast path
                if let Some(ref generic_eng) = view.workspace.generic_engine {
                    if let (Some("textDocument/rename"), Some(req_id)) = (method, &id) {
                        // Servers such as pyright only rename inside open documents:
                        // open every file that references the symbol first.
                        let params = val.get("params").cloned().unwrap_or(serde_json::json!({}));
                        let engine = Arc::clone(generic_eng);
                        let out_tx_task = out_tx.clone();
                        let translator_task = translator.clone();
                        let r_id = req_id.clone();
                        let session_id = view.session_id;
                        let start = Instant::now();
                        TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
                        tokio::task::spawn(async move {
                            let (resp, opened) = rename_with_references_open(&engine, params).await;
                            tracing::info!(
                                session = session_id,
                                opened,
                                duration_ms =
                                    format!("{:.2}ms", start.elapsed().as_secs_f64() * 1000.0),
                                "✅ [LSP DONE] generic rename"
                            );
                            let resp = match resp {
                                Ok(mut resp) => {
                                    send_busy_note(&mut resp, &out_tx_task).await;
                                    resp["id"] = r_id;
                                    resp
                                }
                                Err(err) => {
                                    serde_json::json!({ "jsonrpc": "2.0", "id": r_id, "error": { "code": -32603, "message": err.to_string() } })
                                }
                            };
                            let client_resp =
                                translator_task.translate_lsp_to_client(&resp.to_string());
                            let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
                        });
                        return Flow::Next;
                    }
                    if let (Some("textDocument/diagnostic"), Some(req_id)) = (method, &id) {
                        // Servers without pull diagnostics answer from what they
                        // published for the document.
                        let params = val.get("params").cloned().unwrap_or(serde_json::json!({}));
                        let engine = Arc::clone(generic_eng);
                        let out_tx_task = out_tx.clone();
                        let translator_task = translator.clone();
                        let r_id = req_id.clone();
                        TOTAL_QUERIES.fetch_add(1, Ordering::Relaxed);
                        tokio::task::spawn(async move {
                            let uri = params
                                .get("textDocument")
                                .and_then(|t| t.get("uri"))
                                .and_then(|u| u.as_str())
                                .unwrap_or("")
                                .to_string();
                            let resp = match ManagedLsp::Generic(&engine)
                                .diagnostics_for(&uri)
                                .await
                            {
                                Ok(items) => {
                                    serde_json::json!({ "jsonrpc": "2.0", "id": r_id, "result": { "kind": "full", "items": items } })
                                }
                                // No report is not a clean one (#471).
                                Err(err) => {
                                    serde_json::json!({ "jsonrpc": "2.0", "id": r_id, "error": { "code": -32603, "message": err.to_string() } })
                                }
                            };
                            let client_resp =
                                translator_task.translate_lsp_to_client(&resp.to_string());
                            let _ = out_tx_task.send(WireMessage::LspPayload(client_resp)).await;
                        });
                        return Flow::Next;
                    }
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
                            let resp_res =
                                generic_eng_clone.send_request(&method_str, params).await;
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
                                    send_busy_note(&mut resp, &out_tx_task).await;
                                    resp["id"] = r_id;
                                    let client_resp =
                                        translator_task.translate_lsp_to_client(&resp.to_string());
                                    let _ = out_tx_task
                                        .send(WireMessage::LspPayload(client_resp))
                                        .await;
                                }
                                Err(err) => {
                                    let err_resp = serde_json::json!({
                                        "jsonrpc": "2.0",
                                        "id": r_id,
                                        "error": { "code": -32603, "message": err.to_string() }
                                    });
                                    let _ = out_tx_task
                                        .send(WireMessage::LspPayload(err_resp.to_string()))
                                        .await;
                                }
                            }
                        });
                        return Flow::Next;
                    } else if let Some(m) = method {
                        let params = val.get("params").cloned().unwrap_or(serde_json::json!({}));
                        let _ = generic_eng
                            .send_session_notification(view.session_id, m, params)
                            .await;
                        return Flow::Next;
                    }
                }

                // 6. Handle "textDocument/didClose"
                if let (Some("textDocument/didClose"), Some(backend)) =
                    (method, &view.workspace.backend)
                {
                    let uri = val
                        .get("params")
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
                    let _ = out_tx
                        .send(WireMessage::LspPayload(empty_resp.to_string()))
                        .await;
                    return Flow::Next;
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
            let mut watched = Vec::new();
            let mut failed: Vec<String> = Vec::new();

            for delta in &req.files {
                let target_path = view.workspace.root.join(&delta.relative_path);
                match &delta.content {
                    Some(content_bytes) => {
                        bytes_transferred += content_bytes.len();
                        let kind = if target_path.exists() {
                            workspace::WatchedChange::Changed
                        } else {
                            workspace::WatchedChange::Created
                        };
                        if let Err(e) =
                            write_synced_file(&target_path, content_bytes, delta.is_executable)
                                .await
                        {
                            tracing::warn!(error = %e, file = %target_path.display(), "sync write failed; the client sends it again");
                            failed.push(delta.relative_path.clone());
                            continue;
                        }
                        files_updated += 1;
                        watched.push((target_path.clone(), kind));
                        // The workspace is this worktree's own: synced files are its
                        // new base, visible to every session except one that still
                        // holds an unsaved buffer for the same path.
                        if let Ok(text) = std::str::from_utf8(content_bytes) {
                            for engine_lock in view.workspace.mirrored_rust_engines() {
                                let mut engine = engine_lock.lock().await;
                                let res =
                                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                        engine.update_base(&target_path, Some(text.to_string()))
                                    }));
                                match res {
                                    Ok(Err(e)) => {
                                        tracing::warn!(error = %e, file = %target_path.display(), "base update failed");
                                    }
                                    Err(_) => {
                                        tracing::warn!(file = %target_path.display(), "base update panicked; continuing");
                                    }
                                    Ok(Ok(())) => {}
                                }
                            }
                        }
                    }
                    None => {
                        if target_path.exists()
                            && tokio::fs::remove_file(&target_path).await.is_ok()
                        {
                            files_deleted += 1;
                            watched.push((target_path.clone(), workspace::WatchedChange::Deleted));
                            prune_empty_parents(&view.workspace.root, target_path.parent());
                        }
                        for engine_lock in view.workspace.mirrored_rust_engines() {
                            let mut engine = engine_lock.lock().await;
                            let res =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    engine.update_base(&target_path, None)
                                }));
                            match res {
                                Ok(Err(e)) => {
                                    tracing::warn!(error = %e, file = %target_path.display(), "base removal failed");
                                }
                                Err(_) => {
                                    tracing::warn!(file = %target_path.display(), "base removal panicked; continuing");
                                }
                                Ok(Ok(())) => {}
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
                    Err(e) => {
                        tracing::warn!(error = %e, session = view.session_id, "failed to drop stale session buffers")
                    }
                }
            }

            let mut stale_paths = workspace::clear_stale_paths(
                &view.workspace.root,
                req.files.iter().map(|delta| delta.relative_path.as_str()),
            );
            workspace::record_stale_paths(&view.workspace.root, &failed);
            for path in failed {
                if !stale_paths.contains(&path) {
                    stale_paths.push(path);
                }
            }
            view.workspace.notify_watched_files(&watched).await;
            let duration_ms = start.elapsed().as_millis() as u64;
            let _ = out_tx
                .send(WireMessage::SyncResponse(SyncResponse {
                    files_updated,
                    files_deleted,
                    bytes_transferred,
                    duration_ms,
                    server_workspace_root: view.workspace.root.to_string_lossy().to_string(),
                    workspace_was_fresh: false,
                    stale_paths,
                }))
                .await;
        }
        Some(Ok(WireMessage::Disconnect { reason })) => {
            tracing::info!(reason, "Client terminated session");
            return Flow::Stop;
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
                    load_average_millis: memory::load_average_1m().map(|l| (l * 1000.0) as u32),
                    cpu_count: std::thread::available_parallelism().ok().map(|n| n.get()),
                    platform: Some(prod_code_protocol::platform()),
                    running_commands: running_commands(),
                    host: memory::host_resources(&view.worktree_root),
                    version: Some(env!("CARGO_PKG_VERSION").to_string()),
                    git_commit: Some(prod_code_protocol::git_commit().to_string())
                        .filter(|c| c != "unknown"),
                }))
                .await;
        }
        Some(Ok(WireMessage::ReadFileRequest(req))) => {
            let resp = read_server_file(&meta.storage_root, &req);
            let _ = out_tx.send(WireMessage::ReadFileResponse(resp)).await;
        }
        Some(Err(e)) => {
            tracing::error!(error = %e, "TCP frame decode error");
            return Flow::Stop;
        }
        None => {
            tracing::debug!("Client disconnected");
            return Flow::Stop;
        }
        _ => {}
    }
    Flow::Next
}

#[cfg(test)]
mod tests;
