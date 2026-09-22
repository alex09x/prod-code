//! Workspace file scanner for fast sync over 10G LAN.

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{
    FileDelta, FileStamp, ProdCodeCodec, SyncProbeRequest, SyncRequest, WireMessage, content_hash,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::TcpStream;
use tokio_util::codec::Framed;

const MAX_FILE_SIZE: u64 = 5 * 1024 * 1024; // 5 MiB per source file limit
const MAX_JSON_CONFIG_SIZE: u64 = 256 * 1024; // 256 KiB for .json configs (reject datasets)

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncFileEntry {
    pub mtime_sec: u64,
    pub mtime_nsec: u32,
    pub size: u64,
    #[serde(default)]
    pub hash: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SyncCache {
    #[serde(default)]
    pub base_commit_sha: Option<String>,
    #[serde(default)]
    pub last_sync_timestamp_ms: u64,
    #[serde(default)]
    pub files: HashMap<String, SyncFileEntry>,
    /// Paths that were dirty or untracked at the last full sync. A path that later drops out of
    /// this set without a commit was reverted and must be sent again in its clean form.
    #[serde(default)]
    pub dirty_paths: BTreeSet<String>,
    /// [`RELEVANCE_VERSION`] the watermark was built with. When the relevance filter learns
    /// about new file kinds, older watermarks would wrongly assume those files were sent.
    #[serde(default)]
    pub filter_version: u32,
}

/// Bump whenever [`is_relevant_code_or_manifest_file`] starts accepting more files. A watermark
/// recorded under an older version is treated as first contact, which costs one manifest probe
/// (the gateway then asks only for the files it lacks).
pub const RELEVANCE_VERSION: u32 = 4;

/// How a checkout identifies itself to the gateway.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceIdentity {
    /// Server workspace name. A git worktree gets its own name derived from its origin
    /// repository plus a hash of its path, so every worktree owns an isolated server
    /// workspace and analysis database instead of sharing the origin's.
    pub name: String,
    /// Name of the origin repository when `dir` is a linked worktree.
    pub base: Option<String>,
}

/// Engine the gateway is expected to pick for `root` from its manifest, or `None` when the
/// checkout carries no manifest the gateway keys on. Mirrors the gateway's detection order.
/// The project a path belongs to inside a checkout: the nearest ancestor of `hint` (up to
/// `root`) carrying a manifest of a *different* language than the checkout root. Returns the
/// engine subpath (relative, `/`-separated) and that project's engine; `(None, root engine)`
/// when the path belongs to the root project (nested crates of one Cargo workspace stay
/// with the workspace).
pub fn engine_project(root: &Path, hint: &Path) -> (Option<String>, Option<&'static str>) {
    let root_engine = expected_engine(root);
    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut dir = std::fs::canonicalize(hint).unwrap_or_else(|_| hint.to_path_buf());
    if dir.is_file() {
        dir = dir.parent().map(Path::to_path_buf).unwrap_or(dir);
    }
    while dir.starts_with(&canonical_root) && dir != canonical_root {
        if let Some(engine) = expected_engine(&dir) {
            // Same language is not the same project. A Cargo workspace answers for its
            // members; a crate it excludes belongs to no project the root analyzer loaded, so
            // it needs one of its own or every query in it comes back null.
            if Some(engine) == root_engine && !excluded_from_root_workspace(&canonical_root, &dir) {
                break;
            }
            let rel = dir
                .strip_prefix(&canonical_root)
                .ok()
                .map(|r| {
                    r.components()
                        .map(|c| c.as_os_str().to_string_lossy().into_owned())
                        .collect::<Vec<_>>()
                        .join("/")
                })
                .filter(|r| !r.is_empty());
            return (rel, Some(engine));
        }
        match dir.parent() {
            Some(parent) => dir = parent.to_path_buf(),
            None => break,
        }
    }
    (None, root_engine)
}

/// Is `dir` a Cargo crate that the workspace at `root` does not own?
///
/// Reads the root manifest's `[workspace]` table: a directory listed under `exclude` (by prefix)
/// or absent from a `members` list that has no glob covering it is not part of the workspace's
/// project model, however much it looks like one from the outside.
fn excluded_from_root_workspace(root: &Path, dir: &Path) -> bool {
    if !dir.join("Cargo.toml").is_file() {
        return false;
    }
    let Ok(manifest) = std::fs::read_to_string(root.join("Cargo.toml")) else {
        return false;
    };
    let Ok(rel) = dir.strip_prefix(root) else {
        return false;
    };
    let rel = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/");
    if rel.is_empty() {
        return false;
    }
    let (members, excludes) = workspace_lists(&manifest);
    if excludes
        .iter()
        .any(|e| rel == *e || rel.starts_with(&format!("{e}/")))
    {
        return true;
    }
    // No `members` at all: the manifest is a plain package, and a crate below it is its own.
    if members.is_empty() {
        return !manifest.contains("[workspace]");
    }
    !members.iter().any(|m| match m.strip_suffix("/*") {
        Some(prefix) => {
            rel.starts_with(&format!("{prefix}/")) && rel[prefix.len() + 1..].find('/').is_none()
        }
        None => rel == *m,
    })
}

/// The `members` and `exclude` entries of a root manifest's `[workspace]` table, read without a
/// TOML parser: both are arrays of plain strings, and this only has to recognise them.
fn workspace_lists(manifest: &str) -> (Vec<String>, Vec<String>) {
    let mut members = Vec::new();
    let mut excludes = Vec::new();
    let mut target: Option<&mut Vec<String>> = None;
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            target = None;
        }
        let start = |key: &str| {
            trimmed
                .strip_prefix(key)
                .map(|rest| rest.trim_start().starts_with('='))
                .unwrap_or(false)
        };
        if start("members") {
            members.extend(entries_on(trimmed));
            target = if trimmed.trim_end().ends_with(']') {
                None
            } else {
                Some(&mut members)
            };
            continue;
        }
        if start("exclude") {
            excludes.extend(entries_on(trimmed));
            target = if trimmed.trim_end().ends_with(']') {
                None
            } else {
                Some(&mut excludes)
            };
            continue;
        }
        if let Some(list) = target.as_deref_mut() {
            list.extend(entries_on(trimmed));
            if trimmed.starts_with(']') || trimmed.ends_with(']') {
                target = None;
            }
        }
    }
    (members, excludes)
}

/// The quoted strings on one line of a TOML array.
fn entries_on(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(open) = rest.find('"') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('"') else { break };
        let value = after[..close].trim_matches('/').to_string();
        if !value.is_empty() {
            out.push(value);
        }
        rest = &after[close + 1..];
    }
    out
}

pub fn expected_engine(root: &Path) -> Option<&'static str> {
    if let Some(engine) = engine_at(root) {
        return Some(engine);
    }
    // A repository often keeps its project one directory down (`project/go.mod`,
    // `server/Cargo.toml`). Look one level deep and accept the answer only when every
    // child that has a manifest agrees, so a polyglot monorepo stays "any engine".
    let mut found: Option<&'static str> = None;
    let Ok(entries) = std::fs::read_dir(root) else {
        return None;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !path.is_dir()
            || name.starts_with('.')
            || matches!(
                name.as_ref(),
                "target" | "node_modules" | "vendor" | "build" | "dist"
            )
        {
            continue;
        }
        match (engine_at(&path), found) {
            (Some(engine), None) => found = Some(engine),
            (Some(engine), Some(seen)) if engine != seen => return None,
            _ => {}
        }
    }
    found
}

/// The engine a directory's own manifests ask for.
fn engine_at(root: &Path) -> Option<&'static str> {
    let has = |name: &str| root.join(name).exists();
    let has_xcode = std::fs::read_dir(root)
        .map(|entries| {
            entries.flatten().any(|e| {
                let n = e.file_name();
                let n = n.to_string_lossy();
                n.ends_with(".xcodeproj") || n.ends_with(".xcworkspace")
            })
        })
        .unwrap_or(false);
    if has("Cargo.toml") {
        Some("rust")
    } else if has("go.mod") || has("go.work") {
        Some("go")
    } else if has("Package.swift") || has_xcode {
        Some("swift")
    } else if has("compile_commands.json")
        || has("CMakeLists.txt")
        || has("meson.build")
        || has(".clangd")
    {
        Some("cpp")
    } else if has("pyproject.toml")
        || has("requirements.txt")
        || has("setup.py")
        || has("setup.cfg")
        || has("Pipfile")
    {
        Some("python")
    } else if has("tsconfig.json")
        || has("package.json")
        || has("jsconfig.json")
        || has("deno.json")
        || has("deno.jsonc")
    {
        Some("typescript")
    } else {
        None
    }
}

/// Derives the server workspace identity of `dir`.
pub fn workspace_identity(dir: &Path) -> WorkspaceIdentity {
    let canonical = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let own_name = canonical
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("workspace")
        .to_string();
    let dot_git = canonical.join(".git");
    if dot_git.is_file()
        && let Ok(content) = std::fs::read_to_string(&dot_git)
    {
        for line in content.lines() {
            let Some(gitdir) = line.trim().strip_prefix("gitdir:") else {
                continue;
            };
            let gitdir_path = PathBuf::from(gitdir.trim());
            let mut cur = gitdir_path.as_path();
            while let Some(parent) = cur.parent() {
                if cur.file_name().is_some_and(|n| n == ".git")
                    && let Some(origin) = parent.file_name().and_then(|n| n.to_str())
                {
                    let hash = stable_hash(canonical.to_string_lossy().as_bytes()) as u32;
                    return WorkspaceIdentity {
                        name: format!("{origin}--wt-{hash:08x}"),
                        base: Some(origin.to_string()),
                    };
                }
                cur = parent;
            }
        }
    }
    WorkspaceIdentity {
        name: own_name,
        base: None,
    }
}

fn cache_dir() -> PathBuf {
    let cache_dir = std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".local/share/prod_code/sync"))
        .unwrap_or_else(|| std::env::temp_dir().join("prod_code_sync_cache"));
    let _ = std::fs::create_dir_all(&cache_dir);
    cache_dir
}

fn worktree_cache_id(root: &Path) -> String {
    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    format!(
        "{:016x}",
        stable_hash(canonical_root.to_string_lossy().as_bytes())
    )
}

/// The watermark file for `root` as seen by gateway `node` (`host:port`; empty for the
/// node-less legacy watermark used by tests). Every gateway holds its own copy of the
/// workspace, so what has been uploaded is a per-node fact.
fn cache_file_path(root: &Path, node: &str) -> PathBuf {
    let id = worktree_cache_id(root);
    if node.is_empty() {
        cache_dir().join(format!("{id}.json"))
    } else {
        cache_dir().join(format!("{id}-{:016x}.json", stable_hash(node.as_bytes())))
    }
}

/// All watermark files recorded for `root`, one per gateway node (plus the legacy one).
fn cache_file_paths(root: &Path) -> Vec<PathBuf> {
    let id = worktree_cache_id(root);
    let dir = cache_dir();
    let mut paths = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.ends_with(".json")
                && (name == format!("{id}.json") || name.starts_with(&format!("{id}-")))
            {
                paths.push(entry.path());
            }
        }
    }
    paths
}

pub fn load_sync_cache(root: &Path) -> SyncCache {
    load_sync_cache_for(root, "")
}

pub fn load_sync_cache_for(root: &Path, node: &str) -> SyncCache {
    let path = cache_file_path(root, node);
    if let Ok(data) = std::fs::read(&path) {
        if let Ok(cache) = serde_json::from_slice::<SyncCache>(&data) {
            return cache;
        }
    }
    SyncCache::default()
}

pub fn save_sync_cache(root: &Path, cache: &SyncCache) {
    save_sync_cache_for(root, "", cache)
}

pub fn save_sync_cache_for(root: &Path, node: &str, cache: &SyncCache) {
    let path = cache_file_path(root, node);
    if let Ok(data) = serde_json::to_vec(cache) {
        let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
        if std::fs::write(&temporary, data).is_ok() {
            let _ = std::fs::rename(temporary, path);
        }
    }
}

/// Forgets the watermarks of `root` for every node.
pub fn clear_sync_cache(root: &Path) {
    for path in cache_file_paths(root) {
        let _ = std::fs::remove_file(path);
    }
}

/// Forgets the watermark of `root` for one node only.
pub fn clear_sync_cache_for(root: &Path, node: &str) {
    let _ = std::fs::remove_file(cache_file_path(root, node));
}

/// Drops `rel_paths` from every node's watermark of `root`, so the next sync to any node
/// uploads them again. Used after the client rewrote files itself (refactorings), which no
/// gateway has seen.
pub fn forget_synced_files(root: &Path, rel_paths: &[String]) {
    for path in cache_file_paths(root) {
        let Ok(data) = std::fs::read(&path) else {
            continue;
        };
        let Ok(mut cache) = serde_json::from_slice::<SyncCache>(&data) else {
            continue;
        };
        let mut changed = false;
        for rel in rel_paths {
            changed |= cache.files.remove(rel).is_some();
        }
        if changed && let Ok(bytes) = serde_json::to_vec(&cache) {
            let _ = std::fs::write(&path, bytes);
        }
    }
}

/// A prepared incremental sync. The state is committed only after the remote accepts the files.
#[derive(Debug)]
pub struct SyncPlan {
    pub files: Vec<FileDelta>,
    state: SyncCache,
    /// Gateway node the plan was prepared for (`host:port`), which owns the watermark.
    node: String,
    /// True when this is the worktree's first contact (no recorded base): the plan carries the
    /// complete relevant tree and the gateway should be probed with a manifest first.
    pub initial: bool,
}

impl SyncPlan {
    /// Size/hash stamps of every relevant file this worktree considers synced after the plan.
    pub fn manifest(&self) -> Vec<FileStamp> {
        let mut stamps: Vec<FileStamp> = self
            .state
            .files
            .iter()
            .map(|(path, entry)| FileStamp {
                relative_path: path.clone(),
                size: entry.size,
                hash: entry.hash,
            })
            .collect();
        stamps.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
        stamps
    }

    /// Keeps only the uploads the gateway asked for (deletions are always kept).
    pub fn retain_uploads(&mut self, keep: &HashSet<String>) {
        self.files
            .retain(|f| f.content.is_none() || keep.contains(&f.relative_path));
    }
}

/// What a [`push_workspace_sync`] round did.
#[derive(Debug, Clone, Default)]
pub struct SyncOutcome {
    /// Files the local plan wanted to send before the probe.
    pub planned: usize,
    /// Files actually uploaded.
    pub files_updated: usize,
    pub files_deleted: usize,
    pub bytes_transferred: usize,
    pub probed: bool,
    /// The gateway seeded the workspace from the origin repository's copy.
    pub seeded: bool,
    pub server_workspace_root: String,
    /// Relative paths this round uploaded or deleted on the gateway.
    pub changed_paths: Vec<String>,
}

async fn wait_for_message<T>(
    framed: &mut Framed<TcpStream, ProdCodeCodec>,
    what: &str,
    pick: impl Fn(WireMessage) -> Option<T>,
) -> Result<T> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            anyhow::bail!("timed out waiting for {what}");
        }
        match tokio::time::timeout(remaining, framed.next()).await {
            Ok(Some(Ok(WireMessage::LspPayload(_)))) | Ok(Some(Ok(WireMessage::Pong))) => {}
            Ok(Some(Ok(WireMessage::Disconnect { reason }))) => {
                anyhow::bail!("gateway disconnected while waiting for {what}: {reason}")
            }
            Ok(Some(Ok(msg))) => match pick(msg) {
                Some(value) => return Ok(value),
                None => anyhow::bail!("unexpected message while waiting for {what}"),
            },
            Ok(Some(Err(e))) => anyhow::bail!("frame decode error while waiting for {what}: {e}"),
            Ok(None) => anyhow::bail!("gateway closed connection while waiting for {what}"),
            Err(_) => anyhow::bail!("timed out waiting for {what}"),
        }
    }
}

/// Brings the gateway's copy of `root` up to date on an open, not yet handshaken connection.
///
/// First contact (no recorded base) sends a manifest probe so the gateway can seed the
/// workspace from the origin repository's copy and report only the files it still lacks;
/// later rounds send the watermark delta. The watermark is committed once the gateway has
/// acknowledged the uploads.
pub async fn push_workspace_sync(
    framed: &mut Framed<TcpStream, ProdCodeCodec>,
    root: &Path,
    identity: &WorkspaceIdentity,
    subpath: Option<&Path>,
) -> Result<SyncOutcome> {
    let node = framed
        .get_ref()
        .peer_addr()
        .map(|addr| addr.to_string())
        .unwrap_or_default();
    let mut plan = prepare_workspace_sync_for(root, &node, subpath)?;
    let root_str = root.to_string_lossy().to_string();
    let mut outcome = SyncOutcome {
        planned: plan.files.len(),
        ..SyncOutcome::default()
    };

    if plan.initial && !plan.files.is_empty() {
        framed
            .send(WireMessage::SyncProbeRequest(SyncProbeRequest {
                client_workspace_root: root_str.clone(),
                base_workspace_name: Some(identity.name.clone()),
                seed_from: identity.base.clone(),
                files: plan.manifest(),
            }))
            .await?;
        let probe = wait_for_message(framed, "sync probe response", |m| match m {
            WireMessage::SyncProbeResponse(r) => Some(r),
            _ => None,
        })
        .await?;
        outcome.probed = true;
        outcome.seeded = probe.seeded;
        outcome.files_deleted += probe.files_deleted;
        outcome.server_workspace_root = probe.server_workspace_root;
        let keep: HashSet<String> = probe.missing.into_iter().collect();
        plan.retain_uploads(&keep);
    }

    // An empty delta is still sent on later rounds: the gateway's answer tells whether its
    // copy of the workspace still exists, so a pruned or never-seen node is caught here
    // instead of failing the query or command that follows.
    if !plan.files.is_empty() || !plan.initial {
        let files = std::mem::take(&mut plan.files);
        outcome.changed_paths = files.iter().map(|f| f.relative_path.clone()).collect();
        framed
            .send(WireMessage::SyncRequest(SyncRequest {
                client_workspace_root: root_str,
                files,
                clean_others: false,
                base_workspace_name: Some(identity.name.clone()),
            }))
            .await?;
        let resp = wait_for_message(framed, "sync response", |m| match m {
            WireMessage::SyncResponse(r) => Some(r),
            _ => None,
        })
        .await
        .context("workspace sync was not acknowledged")?;
        if resp.workspace_was_fresh && !plan.initial {
            // The server directory was reset behind our watermark: forget it and start over
            // with a manifest probe on the same connection.
            tracing::warn!(
                workspace = %identity.name,
                "gateway workspace was reset; resyncing the full tree"
            );
            clear_sync_cache_for(root, &node);
            return Box::pin(push_workspace_sync(framed, root, identity, subpath)).await;
        }
        outcome.files_updated = resp.files_updated;
        outcome.files_deleted += resp.files_deleted;
        outcome.bytes_transferred = resp.bytes_transferred;
        outcome.server_workspace_root = resp.server_workspace_root;
    }

    commit_workspace_sync(root, &plan);
    Ok(outcome)
}

/// Build a sync plan from the last acknowledged git base plus the current working tree.
///
/// The first plan for a worktree includes tracked source/manifest files. Later plans use both
/// `git diff <base>` and `git status --porcelain -uall`, then verify candidates against the
/// persisted mtime/size/hash watermark before reading them.
pub fn prepare_workspace_sync(root: &Path, subpath: Option<&Path>) -> Result<SyncPlan> {
    prepare_workspace_sync_for(root, "", subpath)
}

/// [`prepare_workspace_sync`] against the watermark of gateway `node`.
pub fn prepare_workspace_sync_for(
    root: &Path,
    node: &str,
    subpath: Option<&Path>,
) -> Result<SyncPlan> {
    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut state = load_sync_cache_for(&canonical_root, node);
    if state.filter_version != RELEVANCE_VERSION && subpath.is_none() {
        state.base_commit_sha = None;
        state.files.clear();
        state.filter_version = RELEVANCE_VERSION;
    }
    let initial = state.base_commit_sha.is_none() && subpath.is_none();
    let current_base = git_head(&canonical_root)?;
    let (mut changes, current_dirty) = changed_paths(
        &canonical_root,
        state.base_commit_sha.as_deref(),
        &current_base,
    )?;
    // A file that was dirty last time and is clean now without a commit was reverted: the
    // gateway still holds the dirty version, so send the clean one (or its deletion).
    for reverted in state.dirty_paths.difference(&current_dirty) {
        if !changes.contains_key(reverted) {
            let exists = canonical_root.join(reverted).is_file();
            changes.insert(reverted.clone(), !exists);
        }
    }
    let filter = SyncPathFilter::new(&canonical_root, subpath)?;
    let mut files = Vec::new();

    for (relative_path, deleted) in changes {
        if !filter.includes(&relative_path) || !is_relevant_code_or_manifest_file(&relative_path) {
            continue;
        }

        if deleted {
            files.push(FileDelta {
                relative_path: relative_path.clone(),
                content: None,
                is_executable: false,
            });
            state.files.remove(&relative_path);
            continue;
        }

        let full_path = canonical_root.join(&relative_path);
        let Ok(metadata) = full_path.metadata() else {
            continue;
        };
        if !metadata.is_file()
            || metadata.len() > MAX_FILE_SIZE
            || (relative_path.ends_with(".json") && metadata.len() > MAX_JSON_CONFIG_SIZE)
        {
            continue;
        }

        let content = std::fs::read(&full_path)?;
        let entry = sync_file_entry(&metadata, &content);
        if state.files.get(&relative_path) == Some(&entry) {
            continue;
        }

        #[cfg(unix)]
        let is_executable = {
            use std::os::unix::fs::PermissionsExt;
            metadata.permissions().mode() & 0o111 != 0
        };
        #[cfg(not(unix))]
        let is_executable = false;

        files.push(FileDelta {
            relative_path: relative_path.clone(),
            content: Some(content),
            is_executable,
        });
        state.files.insert(relative_path, entry);
    }

    // A partial sync cannot advance the workspace-wide base: changes outside the selected path
    // still need to be included by the next full sync.
    if subpath.is_none() {
        state.base_commit_sha = Some(current_base);
        state.dirty_paths = current_dirty;
    }
    Ok(SyncPlan {
        files,
        state,
        node: node.to_string(),
        initial,
    })
}

/// Writes files a remote command changed into the checkout and records them in the watermark,
/// so the next sync does not push them straight back. Returns the relative paths written or
/// deleted.
pub fn apply_pulled_files(root: &Path, files: &[FileDelta]) -> Result<Vec<String>> {
    apply_pulled_files_for(root, "", files)
}

/// [`apply_pulled_files`] recording the files as synced with gateway `node`, the node that
/// produced them.
pub fn apply_pulled_files_for(root: &Path, node: &str, files: &[FileDelta]) -> Result<Vec<String>> {
    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut state = load_sync_cache_for(&canonical_root, node);
    let mut touched = Vec::with_capacity(files.len());
    for delta in files {
        let rel = Path::new(&delta.relative_path);
        if rel.is_absolute()
            || rel
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            continue; // never let the server write outside the checkout
        }
        let target = canonical_root.join(rel);
        match &delta.content {
            Some(content) => {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&target, content)?;
                #[cfg(unix)]
                if delta.is_executable {
                    use std::os::unix::fs::PermissionsExt;
                    let _ =
                        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755));
                }
                if let Ok(metadata) = target.metadata() {
                    state.files.insert(
                        delta.relative_path.clone(),
                        sync_file_entry(&metadata, content),
                    );
                }
            }
            None => {
                if target.exists() {
                    std::fs::remove_file(&target)?;
                }
                state.files.remove(&delta.relative_path);
            }
        }
        touched.push(delta.relative_path.clone());
    }
    if !touched.is_empty() {
        save_sync_cache_for(&canonical_root, node, &state);
    }
    Ok(touched)
}

/// Persist the watermarks for a sync plan after its files have been accepted by the gateway.
pub fn commit_workspace_sync(root: &Path, plan: &SyncPlan) {
    let mut state = plan.state.clone();
    state.last_sync_timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    save_sync_cache_for(root, &plan.node, &state);
}

fn stable_hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

fn sync_file_entry(metadata: &std::fs::Metadata, content: &[u8]) -> SyncFileEntry {
    let modified = metadata.modified().unwrap_or(UNIX_EPOCH);
    let duration = modified.duration_since(UNIX_EPOCH).unwrap_or_default();
    SyncFileEntry {
        mtime_sec: duration.as_secs(),
        mtime_nsec: duration.subsec_nanos(),
        size: metadata.len(),
        hash: content_hash(content),
    }
}

fn git_head(root: &Path) -> Result<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "HEAD"])
        .output()?;
    if !output.status.success() {
        anyhow::bail!("git rev-parse HEAD non-zero exit");
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Paths changed since `base` (committed, dirty and untracked) mapped to "deleted", plus the
/// set of paths that are currently dirty or untracked.
fn changed_paths(
    root: &Path,
    base: Option<&str>,
    head: &str,
) -> Result<(BTreeMap<String, bool>, BTreeSet<String>)> {
    let mut paths = BTreeMap::new();
    // A recorded base can disappear after a rebase, amend or gc; fall back to the full tracked
    // tree instead of failing the sync, since the watermarks still filter unchanged files.
    // When HEAD has not moved since the recorded base there are no committed changes to
    // list, so the diff (the most expensive git call of the round) is skipped entirely.
    let diff_from_base = match base {
        Some(base) if base == head => Some(Vec::new()),
        Some(base) if !base.is_empty() => {
            git_output(root, ["diff", "--name-status", "-z", base]).ok()
        }
        _ => None,
    };
    match diff_from_base {
        Some(output) => parse_name_status(&output, &mut paths),
        None => {
            let output = git_output(root, ["ls-files", "-z"])?;
            for path in output
                .split(|byte| *byte == 0)
                .filter(|path| !path.is_empty())
            {
                paths.insert(String::from_utf8_lossy(path).to_string(), false);
            }
        }
    }

    let output = git_output(root, ["status", "--porcelain=v1", "-z", "-uall"])?;
    let mut dirty = BTreeMap::new();
    parse_porcelain_status(&output, &mut dirty);
    let dirty_set: BTreeSet<String> = dirty.keys().cloned().collect();
    paths.extend(dirty);
    Ok((paths, dirty_set))
}

fn git_output<const N: usize>(root: &Path, args: [&str; N]) -> Result<Vec<u8>> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()?;
    if !output.status.success() {
        anyhow::bail!("git command non-zero exit");
    }
    Ok(output.stdout)
}

fn parse_name_status(output: &[u8], paths: &mut BTreeMap<String, bool>) {
    let mut fields = output
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty());
    while let Some(status) = fields.next() {
        let status = String::from_utf8_lossy(status);
        let Some(path) = fields.next() else { break };
        if status.starts_with('R') || status.starts_with('C') {
            let Some(new_path) = fields.next() else { break };
            paths.insert(String::from_utf8_lossy(path).to_string(), true);
            paths.insert(String::from_utf8_lossy(new_path).to_string(), false);
        } else {
            paths.insert(
                String::from_utf8_lossy(path).to_string(),
                status.starts_with('D'),
            );
        }
    }
}

fn parse_porcelain_status(output: &[u8], paths: &mut BTreeMap<String, bool>) {
    let mut fields = output
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty());
    while let Some(entry) = fields.next() {
        if entry.len() < 4 {
            continue;
        }
        let status = &entry[..2];
        let path = String::from_utf8_lossy(&entry[3..]).to_string();
        if status.contains(&b'R') || status.contains(&b'C') {
            let Some(old_path) = fields.next() else { break };
            paths.insert(String::from_utf8_lossy(old_path).to_string(), true);
        }
        paths.insert(path, status.contains(&b'D'));
    }
}

struct SyncPathFilter {
    relative: Option<PathBuf>,
}

impl SyncPathFilter {
    fn new(root: &Path, subpath: Option<&Path>) -> Result<Self> {
        let relative = match subpath {
            Some(path) => {
                let path = if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    root.join(path)
                };
                let path = std::fs::canonicalize(&path).unwrap_or(path);
                Some(path.strip_prefix(root)?.to_path_buf())
            }
            None => None,
        };
        Ok(Self { relative })
    }

    fn includes(&self, relative_path: &str) -> bool {
        self.relative.as_ref().is_none_or(|filter| {
            Path::new(relative_path) == filter || Path::new(relative_path).starts_with(filter)
        })
    }
}

/// Returns true if the relative path represents a code or configuration file relevant to language servers.
pub fn is_relevant_code_or_manifest_file(rel_path: &str) -> bool {
    let path = Path::new(rel_path);
    // Build and tool manifests whose extension alone would not qualify them.
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if matches!(
        file_name,
        "CMakeLists.txt"
            | "CMakePresets.json"
            | "compile_commands.json"
            | "meson.build"
            | "meson_options.txt"
            | "requirements.txt"
            | "requirements-dev.txt"
            | "constraints.txt"
            | "pytest.ini"
            | "tox.ini"
            | "setup.cfg"
            | "mypy.ini"
            | "project.pbxproj"
            | "Podfile"
            | "Package.resolved"
            | ".clangd"
            | ".clang-format"
            | ".clang-tidy"
            | "Pipfile"
            | "BUILD"
            | "WORKSPACE"
            | "rustc-wrapper"
            | "rustc_wrapper"
            | "cargo-wrapper"
    ) || (file_name.starts_with("requirements") && file_name.ends_with(".txt"))
        || file_name.ends_with(".sh")
    {
        return true;
    }

    // 1. Check directory components for non-code / build / data trees
    let mut under_code_dir = false;
    for component in path.components() {
        if let std::path::Component::Normal(comp) = component {
            let s = comp.to_string_lossy();
            if s.starts_with('.') && s != ".cargo" {
                return false;
            }
            if matches!(s.as_ref(), "crates" | "packages" | "src") {
                under_code_dir = true;
            }
            if !under_code_dir
                && matches!(
                    s.as_ref(),
                    "target"
                        | "node_modules"
                        | "vendor"
                        | "dist"
                        | "build"
                        | "results"
                        | "samples"
                        | "__pycache__"
                        | "artifacts"
                        | "dogfood-output"
                        | "data"
                        | "dataset"
                        | "datasets"
                        | "corpus"
                        | "traces"
                        | "state"
                        | "research"
                        | "benchmarks"
                        | "benchmark"
                        | ".idea"
                        | ".vscode"
                )
            {
                return false;
            }
            if under_code_dir && matches!(s.as_ref(), "target" | "node_modules" | "__pycache__") {
                return false;
            }
        }
    }

    // 2. Binary / media extensions
    if is_binary_or_media_file(rel_path) {
        return false;
    }

    // 3. Known non-code data, dumps, documentation, and log formats
    let lower = rel_path.to_lowercase();
    if lower.ends_with(".jsonl")
        || lower.ends_with(".csv")
        || lower.ends_with(".tsv")
        || lower.ends_with(".parquet")
        || lower.ends_with(".arrow")
        || lower.ends_with(".feather")
        || lower.ends_with(".log")
        || lower.ends_with(".md")
        || lower.ends_with(".txt")
        || lower.ends_with(".rst")
        || lower.ends_with(".pdf")
        || lower.ends_with(".doc")
        || lower.ends_with(".docx")
    {
        return false;
    }

    // 4. Code & manifest extensions
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        let ext_lower = ext.to_lowercase();
        matches!(
            ext_lower.as_str(),
            "rs" | "go"
                | "mod"
                | "sum"
                | "work"
                | "py"
                | "pyi"
                | "js"
                | "mjs"
                | "cjs"
                | "jsx"
                | "ts"
                | "mts"
                | "cts"
                | "tsx"
                | "vue"
                | "svelte"
                | "c"
                | "h"
                | "cc"
                | "cpp"
                | "cxx"
                | "hh"
                | "hpp"
                | "hxx"
                | "inl"
                | "java"
                | "kt"
                | "kts"
                | "scala"
                | "cs"
                | "swift"
                | "proto"
                | "thrift"
                | "graphql"
                | "gql"
                | "sql"
                | "sh"
                | "bash"
                | "zsh"
                | "toml"
                | "lock" // Cargo.lock, yarn.lock, poetry.lock: pin what the server builds
                | "yaml"
                | "yml"
                | "json"
        )
    } else {
        // Files without extension: manifests and scripts
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        matches!(
            file_name,
            "Makefile"
                | "Dockerfile"
                | "Containerfile"
                | "Procfile"
                | "Gemfile"
                | "Rakefile"
                | "Cargo.lock"
                | ".clangd"
                | ".clang-format"
                | ".clang-tidy"
                | "Pipfile"
                | "BUILD"
                | "WORKSPACE"
        )
    }
}

/// Scan workspace directory and generate FileDelta list, filtering out build artifacts and VCS.
pub fn scan_workspace_files(root: &Path, subpath: Option<&Path>) -> Result<Vec<FileDelta>> {
    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let target_dir = match subpath {
        Some(sub) => {
            if sub.is_absolute() {
                std::fs::canonicalize(sub).unwrap_or_else(|_| sub.to_path_buf())
            } else {
                let joined = canonical_root.join(sub);
                std::fs::canonicalize(&joined).unwrap_or(joined)
            }
        }
        None => canonical_root.clone(),
    };

    if !target_dir.exists() {
        anyhow::bail!("Path {:?} does not exist", target_dir);
    }

    let mut deltas = Vec::new();

    if target_dir.is_file() {
        let rel_path = target_dir
            .strip_prefix(&canonical_root)
            .unwrap_or(&target_dir)
            .to_string_lossy()
            .to_string();
        if is_relevant_code_or_manifest_file(&rel_path) {
            let content = std::fs::read(&target_dir)?;
            deltas.push(FileDelta {
                relative_path: rel_path,
                content: Some(content),
                is_executable: false,
            });
        }
        return Ok(deltas);
    }

    walk_dir(&target_dir, &canonical_root, &mut deltas)?;
    Ok(deltas)
}

/// Collect dirty, modified, untracked, and deleted files in a workspace directory.
/// When in a git repository or worktree, uses `git status --porcelain -uall` for sub-10ms discovery.
/// Uses lightweight memoization cache so files that were already synced and unchanged are not re-read or re-sent.
pub fn collect_dirty_files(root: &Path) -> Result<Vec<FileDelta>> {
    // The complete dirty/untracked set, every time. A gateway session keeps these files as its
    // private overlay, so each new session must announce all of them; a persistent
    // "already sent" cache would silently drop them for the second connection onwards.
    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    Ok(collect_git_dirty_files(&canonical_root, false).unwrap_or_default())
}

/// Like [`collect_dirty_files`] but skips files whose mtime/size/hash watermark is already
/// recorded in the persistent per-worktree sync state. Only correct against a server that keeps
/// previously synced files, i.e. the disk-backed base workspace, not session overlays.
pub fn collect_dirty_files_incremental(root: &Path) -> Result<Vec<FileDelta>> {
    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    Ok(collect_git_dirty_files(&canonical_root, true).unwrap_or_default())
}

fn collect_git_dirty_files(root: &Path, use_cache: bool) -> Result<Vec<FileDelta>> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("status")
        .arg("--porcelain")
        .arg("-uall")
        .output()?;

    if !output.status.success() {
        anyhow::bail!("git status non-zero exit");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut deltas = Vec::new();
    let mut cache = if use_cache {
        load_sync_cache(root)
    } else {
        SyncCache::default()
    };
    let mut cache_modified = false;

    for line in stdout.lines() {
        if line.len() < 4 {
            continue;
        }
        let status = &line[..2];
        let raw_path = line[3..].trim();

        let (is_delete, rel_path, old_path) = if status.contains('D') {
            (true, raw_path, None)
        } else if let Some((old_p, new_p)) = raw_path.split_once(" -> ") {
            (
                false,
                new_p.trim().trim_matches('"'),
                Some(old_p.trim().trim_matches('"')),
            )
        } else {
            (false, raw_path.trim_matches('"'), None)
        };

        if let Some(old) = old_path {
            if !old.starts_with(".git") {
                deltas.push(FileDelta {
                    relative_path: old.to_string(),
                    content: None,
                    is_executable: false,
                });
                if cache.files.remove(old).is_some() {
                    cache_modified = true;
                }
            }
        }

        if !is_relevant_code_or_manifest_file(rel_path) {
            continue;
        }

        let full_path = root.join(rel_path);
        if is_delete {
            deltas.push(FileDelta {
                relative_path: rel_path.to_string(),
                content: None,
                is_executable: false,
            });
            if cache.files.remove(rel_path).is_some() {
                cache_modified = true;
            }
        } else if full_path.is_file() {
            if let Ok(metadata) = full_path.metadata() {
                let size = metadata.len();
                if size > MAX_FILE_SIZE {
                    continue;
                }
                if rel_path.ends_with(".json") && size > MAX_JSON_CONFIG_SIZE {
                    continue;
                }

                #[cfg(unix)]
                let is_executable = {
                    use std::os::unix::fs::PermissionsExt;
                    metadata.permissions().mode() & 0o111 != 0
                };
                #[cfg(not(unix))]
                let is_executable = false;

                if let Ok(content) = std::fs::read(&full_path) {
                    let entry = sync_file_entry(&metadata, &content);
                    if cache.files.get(rel_path) == Some(&entry) {
                        // File was already synced and has not changed.
                        continue;
                    }
                    deltas.push(FileDelta {
                        relative_path: rel_path.to_string(),
                        content: Some(content),
                        is_executable,
                    });
                    cache.files.insert(rel_path.to_string(), entry);
                    cache_modified = true;
                }
            }
        }
    }

    if use_cache && cache_modified {
        save_sync_cache(root, &cache);
    }

    Ok(deltas)
}

fn is_binary_or_media_file(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.ends_with(".zip")
        || lower.ends_with(".tar")
        || lower.ends_with(".gz")
        || lower.ends_with(".bin")
        || lower.ends_with(".png")
        || lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
        || lower.ends_with(".pdf")
        || lower.ends_with(".wasm")
        || lower.ends_with(".so")
        || lower.ends_with(".dylib")
        || lower.ends_with(".a")
        || lower.ends_with(".o")
        || lower.ends_with(".exe")
        || lower.ends_with(".hprof")
        || lower.ends_with(".mp4")
        || lower.ends_with(".mov")
        || lower.ends_with(".pyc")
        || lower.ends_with(".db")
        || lower.ends_with(".sqlite")
}

fn walk_dir(target_dir: &Path, canonical_root: &Path, deltas: &mut Vec<FileDelta>) -> Result<()> {
    let mut builder = ignore::WalkBuilder::new(target_dir);
    builder
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .parents(true)
        .max_filesize(Some(MAX_FILE_SIZE))
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            let path = entry.path();
            let is_under_code = path.components().any(|c| {
                if let std::path::Component::Normal(p) = c {
                    matches!(p.to_string_lossy().as_ref(), "crates" | "packages" | "src")
                } else {
                    false
                }
            });

            if !is_under_code
                && matches!(
                    name.as_ref(),
                    "research"
                        | "benchmarks"
                        | "benchmark"
                        | "data"
                        | "dataset"
                        | "datasets"
                        | "corpus"
                        | "traces"
                )
            {
                return false;
            }

            if name == ".git"
                || name == "target"
                || name == "node_modules"
                || name == "vendor"
                || name == "dist"
                || name == "build"
                || name == "results"
                || name == "samples"
                || name == "__pycache__"
                || name == "artifacts"
                || name == "dogfood-output"
                || name == "state"
                || name == ".idea"
                || name == ".vscode"
                || name == ".DS_Store"
            {
                return false;
            }
            true
        });

    for result in builder.build() {
        let entry = match result {
            Ok(e) => e,
            Err(_) => continue,
        };

        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let rel_path = path
            .strip_prefix(canonical_root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();

        if !is_relevant_code_or_manifest_file(&rel_path) {
            continue;
        }

        if let Ok(metadata) = entry.metadata() {
            if rel_path.ends_with(".json") && metadata.len() > MAX_JSON_CONFIG_SIZE {
                continue;
            }
        }

        if let Ok(content) = std::fs::read(path) {
            #[cfg(unix)]
            let is_executable = {
                use std::os::unix::fs::PermissionsExt;
                entry
                    .metadata()
                    .ok()
                    .map(|m| m.permissions().mode() & 0o111 != 0)
                    .unwrap_or(false)
            };
            #[cfg(not(unix))]
            let is_executable = false;

            deltas.push(FileDelta {
                relative_path: rel_path,
                content: Some(content),
                is_executable,
            });
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scan_workspace_files() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();

        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(root.join("Cargo.toml"), "[package]").unwrap();
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::write(root.join("target/debug/app"), "binary").unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".git/config"), "git").unwrap();

        let deltas = scan_workspace_files(root, None).unwrap();
        let paths: Vec<_> = deltas.iter().map(|d| d.relative_path.as_str()).collect();

        assert!(paths.contains(&"src/main.rs") || paths.contains(&"src\\main.rs"));
        assert!(paths.contains(&"Cargo.toml"));
        assert!(!paths.iter().any(|p| p.contains("target")));
        assert!(!paths.iter().any(|p| p.contains(".git")));
    }

    #[test]
    fn test_collect_dirty_files_in_git_repo() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();

        // Initialize a git repo in tempdir
        let init_status = std::process::Command::new("git")
            .arg("init")
            .current_dir(root)
            .status();
        if init_status.is_err() || !init_status.unwrap().success() {
            return; // git not available in environment, skip
        }

        // Configure git user for commit
        let _ = std::process::Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(root)
            .status();
        let _ = std::process::Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(root)
            .status();

        // 1. Initial committed file
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn original() {}").unwrap();
        let _ = std::process::Command::new("git")
            .args(["add", "src/lib.rs"])
            .current_dir(root)
            .status();
        let _ = std::process::Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(root)
            .status();

        // 2. Modify existing file
        std::fs::write(root.join("src/lib.rs"), "pub fn modified() {}").unwrap();

        // 3. Create a brand new untracked file (NO git add)
        std::fs::write(root.join("src/untracked.rs"), "pub fn untracked() {}").unwrap();

        // 4. Create non-code / research files that MUST be ignored
        std::fs::create_dir_all(root.join("research")).unwrap();
        std::fs::write(root.join("research/bench.jsonl"), "{\"dump\": true}").unwrap();
        std::fs::write(root.join("notes.md"), "# Research Notes").unwrap();

        // 5. First collection: code files collected, non-code files ignored
        let deltas = collect_dirty_files_incremental(root).unwrap();
        let map: std::collections::HashMap<_, _> = deltas
            .into_iter()
            .map(|d| (d.relative_path, d.content))
            .collect();

        assert!(map.contains_key("src/lib.rs") || map.contains_key("src\\lib.rs"));
        assert!(map.contains_key("src/untracked.rs") || map.contains_key("src\\untracked.rs"));
        assert!(!map.contains_key("research/bench.jsonl"));
        assert!(!map.contains_key("notes.md"));

        let untracked_content = map
            .get("src/untracked.rs")
            .or_else(|| map.get("src\\untracked.rs"))
            .unwrap()
            .as_ref()
            .unwrap();
        assert_eq!(
            std::str::from_utf8(untracked_content).unwrap(),
            "pub fn untracked() {}"
        );

        // 6. Second consecutive collection without changes: cache hit, zero deltas!
        let deltas_cached = collect_dirty_files_incremental(root).unwrap();
        assert!(
            deltas_cached.is_empty(),
            "Expected 0 deltas on cache hit, got {}",
            deltas_cached.len()
        );

        // 7. Touch one file: only that file is collected again
        std::fs::write(root.join("src/lib.rs"), "pub fn modified_v2() {}").unwrap();
        let deltas_recheck = collect_dirty_files_incremental(root).unwrap();
        assert_eq!(deltas_recheck.len(), 1);
        assert!(deltas_recheck[0].relative_path.contains("lib.rs"));
    }

    #[test]
    fn test_workspace_sync_state_uses_base_commit_and_watermarks() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        clear_sync_cache(root);

        let init = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(root)
            .status()
            .unwrap();
        if !init.success() {
            return;
        }
        for (key, value) in [("user.name", "Test"), ("user.email", "test@example.com")] {
            let configured = std::process::Command::new("git")
                .args(["config", key, value])
                .current_dir(root)
                .status()
                .unwrap();
            assert!(configured.success());
        }

        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn version() -> u8 { 1 }").unwrap();
        for args in [
            &["add", "src/lib.rs"][..],
            &["commit", "-qm", "initial"][..],
        ] {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(root)
                .status()
                .unwrap();
            assert!(status.success());
        }

        let first = prepare_workspace_sync(root, None).unwrap();
        assert_eq!(first.files.len(), 1);
        commit_workspace_sync(root, &first);
        let state = load_sync_cache(root);
        assert!(state.base_commit_sha.is_some());
        assert!(state.last_sync_timestamp_ms > 0);
        assert!(
            state
                .files
                .get("src/lib.rs")
                .is_some_and(|entry| entry.hash != 0)
        );

        let unchanged = prepare_workspace_sync(root, None).unwrap();
        assert!(unchanged.files.is_empty());

        std::fs::write(root.join("src/lib.rs"), "pub fn version() -> u8 { 2 }").unwrap();
        for args in [
            &["add", "src/lib.rs"][..],
            &["commit", "-qm", "changed"][..],
        ] {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(root)
                .status()
                .unwrap();
            assert!(status.success());
        }

        let changed = prepare_workspace_sync(root, None).unwrap();
        assert_eq!(changed.files.len(), 1);
        assert_eq!(changed.files[0].relative_path, "src/lib.rs");
        clear_sync_cache(root);
    }

    #[test]
    fn test_unreachable_base_commit_falls_back_to_full_tree() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        clear_sync_cache(root);

        let init = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(root)
            .status()
            .unwrap();
        if !init.success() {
            return;
        }
        for (key, value) in [("user.name", "Test"), ("user.email", "test@example.com")] {
            let configured = std::process::Command::new("git")
                .args(["config", key, value])
                .current_dir(root)
                .status()
                .unwrap();
            assert!(configured.success());
        }

        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn version() -> u8 { 1 }").unwrap();
        for args in [&["add", "src"][..], &["commit", "-qm", "initial"][..]] {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(root)
                .status()
                .unwrap();
            assert!(status.success());
        }
        let initial = prepare_workspace_sync(root, None).unwrap();
        commit_workspace_sync(root, &initial);

        // Simulate history rewritten underneath the persisted watermark.
        let canonical = std::fs::canonicalize(root).unwrap();
        let mut state = load_sync_cache(&canonical);
        state.base_commit_sha = Some("0123456789abcdef0123456789abcdef01234567".to_string());
        save_sync_cache(&canonical, &state);

        // Unchanged file: fallback scans the full tree but the watermark still filters it.
        let unchanged = prepare_workspace_sync(root, None).unwrap();
        assert!(unchanged.files.is_empty());

        // Changed file: fallback still finds it.
        std::fs::write(root.join("src/lib.rs"), "pub fn version() -> u8 { 3 }").unwrap();
        let changed = prepare_workspace_sync(root, None).unwrap();
        assert_eq!(changed.files.len(), 1);
        assert_eq!(changed.files[0].relative_path, "src/lib.rs");
        clear_sync_cache(root);
    }

    #[test]
    fn test_initial_plan_carries_manifest_and_retains_only_missing() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        clear_sync_cache(root);
        let init = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(root)
            .status()
            .unwrap();
        if !init.success() {
            return;
        }
        for (key, value) in [("user.name", "Test"), ("user.email", "test@example.com")] {
            assert!(
                std::process::Command::new("git")
                    .args(["config", key, value])
                    .current_dir(root)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.rs"), "pub fn a() {}").unwrap();
        std::fs::write(root.join("src/b.rs"), "pub fn b() {}").unwrap();
        for args in [&["add", "src"][..], &["commit", "-qm", "initial"][..]] {
            assert!(
                std::process::Command::new("git")
                    .args(args)
                    .current_dir(root)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let mut plan = prepare_workspace_sync(root, None).unwrap();
        assert!(plan.initial);
        let manifest = plan.manifest();
        assert_eq!(manifest.len(), 2);
        assert_eq!(manifest[0].relative_path, "src/a.rs");
        assert_eq!(manifest[0].hash, content_hash(b"pub fn a() {}"));
        assert_eq!(manifest[0].size, 13);

        let keep: HashSet<String> = ["src/b.rs".to_string()].into_iter().collect();
        plan.retain_uploads(&keep);
        assert_eq!(plan.files.len(), 1);
        assert_eq!(plan.files[0].relative_path, "src/b.rs");
        commit_workspace_sync(root, &plan);

        // The skipped file counts as synced: the next plan is a pure delta and not initial.
        let next = prepare_workspace_sync(root, None).unwrap();
        assert!(!next.initial);
        assert!(next.files.is_empty());
        clear_sync_cache(root);
    }

    #[test]
    fn test_stale_filter_version_forces_manifest_probe() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        clear_sync_cache(root);
        let init = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(root)
            .status()
            .unwrap();
        if !init.success() {
            return;
        }
        for (key, value) in [("user.name", "Test"), ("user.email", "test@example.com")] {
            assert!(
                std::process::Command::new("git")
                    .args(["config", key, value])
                    .current_dir(root)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        std::fs::write(root.join("Cargo.lock"), "# lock").unwrap();
        std::fs::write(root.join("a.rs"), "pub fn a() {}").unwrap();
        for args in [&["add", "."][..], &["commit", "-qm", "initial"][..]] {
            assert!(
                std::process::Command::new("git")
                    .args(args)
                    .current_dir(root)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        // A watermark written by an older client that never sent Cargo.lock.
        let first = prepare_workspace_sync(root, None).unwrap();
        commit_workspace_sync(root, &first);
        let canonical = std::fs::canonicalize(root).unwrap();
        let mut old = load_sync_cache(&canonical);
        old.filter_version = 0;
        old.files.remove("Cargo.lock");
        save_sync_cache(&canonical, &old);

        let upgraded = prepare_workspace_sync(root, None).unwrap();
        assert!(
            upgraded.initial,
            "old watermark must be treated as first contact"
        );
        assert!(
            upgraded
                .files
                .iter()
                .any(|f| f.relative_path == "Cargo.lock"),
            "{upgraded:?}"
        );
        commit_workspace_sync(root, &upgraded);
        assert_eq!(
            load_sync_cache(&canonical).filter_version,
            RELEVANCE_VERSION
        );
        assert!(!prepare_workspace_sync(root, None).unwrap().initial);
        clear_sync_cache(root);
    }

    #[test]
    fn test_apply_pulled_files_writes_and_records_watermark() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        clear_sync_cache(root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/old.rs"), "x").unwrap();
        let files = vec![
            FileDelta {
                relative_path: "src/fmt.rs".to_string(),
                content: Some(b"fn f() {}\n".to_vec()),
                is_executable: false,
            },
            FileDelta {
                relative_path: "src/old.rs".to_string(),
                content: None,
                is_executable: false,
            },
            FileDelta {
                relative_path: "../escape.rs".to_string(),
                content: Some(b"no".to_vec()),
                is_executable: false,
            },
        ];
        let touched = apply_pulled_files(root, &files).unwrap();
        assert_eq!(
            touched,
            vec!["src/fmt.rs".to_string(), "src/old.rs".to_string()]
        );
        assert_eq!(
            std::fs::read_to_string(root.join("src/fmt.rs")).unwrap(),
            "fn f() {}\n"
        );
        assert!(!root.join("src/old.rs").exists());
        assert!(!root.join("../escape.rs").exists());
        let state = load_sync_cache(&std::fs::canonicalize(root).unwrap());
        assert_eq!(
            state.files.get("src/fmt.rs").map(|e| e.hash),
            Some(content_hash(b"fn f() {}\n"))
        );
        clear_sync_cache(root);
    }

    #[test]
    fn nested_project_of_another_language_gets_a_subpath() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::write(root.join("Cargo.toml"), "[workspace]\n").unwrap();
        std::fs::create_dir_all(root.join("crates/a/src")).unwrap();
        std::fs::write(root.join("crates/a/Cargo.toml"), "[package]\n").unwrap();
        std::fs::write(root.join("crates/a/src/lib.rs"), "").unwrap();
        std::fs::create_dir_all(root.join("swift/Sources/App")).unwrap();
        std::fs::write(root.join("swift/Package.swift"), "").unwrap();
        std::fs::write(root.join("swift/Sources/App/main.swift"), "").unwrap();
        assert_eq!(
            engine_project(root, &root.join("crates/a/src/lib.rs")),
            (None, Some("rust"))
        );
        assert_eq!(
            engine_project(root, &root.join("swift/Sources/App/main.swift")),
            (Some("swift".to_string()), Some("swift"))
        );
        assert_eq!(engine_project(root, root), (None, Some("rust")));
    }

    #[test]
    fn test_lockfiles_are_relevant() {
        assert!(is_relevant_code_or_manifest_file("Cargo.lock"));
        assert!(is_relevant_code_or_manifest_file("CMakeLists.txt"));
        assert!(is_relevant_code_or_manifest_file(
            "build/compile_commands.json"
        ));
        assert!(is_relevant_code_or_manifest_file(".clangd"));
        assert!(is_relevant_code_or_manifest_file("requirements.txt"));
        assert!(is_relevant_code_or_manifest_file(
            "App.xcodeproj/project.pbxproj"
        ));
        assert!(!is_relevant_code_or_manifest_file("notes.txt"));
        assert!(is_relevant_code_or_manifest_file("web/yarn.lock"));
        assert!(is_relevant_code_or_manifest_file("go.sum"));
        assert!(is_relevant_code_or_manifest_file("scripts/rustc-wrapper"));
        assert!(is_relevant_code_or_manifest_file(
            "scripts/collect-report.sh"
        ));
    }

    #[test]
    fn test_first_sync_includes_modified_tracked_file() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        clear_sync_cache(root);
        let init = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(root)
            .status()
            .unwrap();
        if !init.success() {
            return;
        }
        for (key, value) in [("user.name", "Test"), ("user.email", "test@example.com")] {
            assert!(
                std::process::Command::new("git")
                    .args(["config", key, value])
                    .current_dir(root)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.rs"), "pub fn a() -> u8 { 1 }").unwrap();
        std::fs::write(root.join("src/b.rs"), "pub fn b() -> u8 { 1 }").unwrap();
        for args in [&["add", "src"][..], &["commit", "-qm", "initial"][..]] {
            assert!(
                std::process::Command::new("git")
                    .args(args)
                    .current_dir(root)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        // Modified before any sync ever happened: must be sent with its modified content.
        std::fs::write(
            root.join("src/a.rs"),
            "pub fn a(divergent_marker: i64) -> u8 { 1 }",
        )
        .unwrap();
        let first = prepare_workspace_sync(root, None).unwrap();
        let mut names: Vec<&str> = first
            .files
            .iter()
            .map(|f| f.relative_path.as_str())
            .collect();
        names.sort();
        assert_eq!(names, vec!["src/a.rs", "src/b.rs"], "{first:?}");
        let a = first
            .files
            .iter()
            .find(|f| f.relative_path == "src/a.rs")
            .unwrap();
        assert!(
            std::str::from_utf8(a.content.as_deref().unwrap())
                .unwrap()
                .contains("divergent_marker")
        );
        clear_sync_cache(root);
    }

    #[test]
    fn test_reverted_dirty_file_is_resent_clean() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        clear_sync_cache(root);
        let init = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(root)
            .status()
            .unwrap();
        if !init.success() {
            return;
        }
        for (key, value) in [("user.name", "Test"), ("user.email", "test@example.com")] {
            assert!(
                std::process::Command::new("git")
                    .args(["config", key, value])
                    .current_dir(root)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn version() -> u8 { 1 }").unwrap();
        for args in [&["add", "src"][..], &["commit", "-qm", "initial"][..]] {
            assert!(
                std::process::Command::new("git")
                    .args(args)
                    .current_dir(root)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let initial = prepare_workspace_sync(root, None).unwrap();
        commit_workspace_sync(root, &initial);

        // Dirty edit is sent and remembered as dirty.
        std::fs::write(root.join("src/lib.rs"), "pub fn version() -> u8 { 2 }").unwrap();
        let dirty = prepare_workspace_sync(root, None).unwrap();
        assert_eq!(dirty.files.len(), 1);
        commit_workspace_sync(root, &dirty);
        assert!(
            load_sync_cache(&std::fs::canonicalize(root).unwrap())
                .dirty_paths
                .contains("src/lib.rs")
        );

        // Revert to HEAD: git reports nothing, yet the clean content must go out again.
        std::fs::write(root.join("src/lib.rs"), "pub fn version() -> u8 { 1 }").unwrap();
        let reverted = prepare_workspace_sync(root, None).unwrap();
        assert_eq!(reverted.files.len(), 1);
        assert_eq!(
            reverted.files[0].content.as_deref(),
            Some("pub fn version() -> u8 { 1 }".as_bytes())
        );
        commit_workspace_sync(root, &reverted);
        assert!(prepare_workspace_sync(root, None).unwrap().files.is_empty());
        clear_sync_cache(root);
    }

    #[test]
    fn engine_is_found_one_directory_down() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("project")).unwrap();
        std::fs::write(root.path().join("project/go.mod"), "module x\n").unwrap();
        assert_eq!(expected_engine(root.path()), Some("go"));

        // Two children asking for different engines is not one answer.
        std::fs::create_dir_all(root.path().join("server")).unwrap();
        std::fs::write(root.path().join("server/Cargo.toml"), "[package]\n").unwrap();
        assert_eq!(expected_engine(root.path()), None);

        // A manifest at the root still wins outright.
        std::fs::write(root.path().join("Cargo.toml"), "[package]\n").unwrap();
        assert_eq!(expected_engine(root.path()), Some("rust"));
    }

    #[test]
    fn test_expected_engine_follows_manifest() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(expected_engine(temp.path()), None);
        std::fs::write(temp.path().join("tsconfig.json"), "{}").unwrap();
        assert_eq!(expected_engine(temp.path()), Some("typescript"));
        std::fs::write(temp.path().join("requirements.txt"), "").unwrap();
        assert_eq!(expected_engine(temp.path()), Some("python"));
        std::fs::write(temp.path().join("CMakeLists.txt"), "").unwrap();
        assert_eq!(expected_engine(temp.path()), Some("cpp"));
        std::fs::write(temp.path().join("Package.swift"), "").unwrap();
        assert_eq!(expected_engine(temp.path()), Some("swift"));
        std::fs::write(temp.path().join("go.mod"), "module x\n").unwrap();
        assert_eq!(expected_engine(temp.path()), Some("go"));
        std::fs::write(temp.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        assert_eq!(expected_engine(temp.path()), Some("rust"));
    }

    #[test]
    fn test_workspace_identity_isolates_worktrees() {
        let temp = tempfile::tempdir().unwrap();
        let origin = temp.path().join("my-repo");
        std::fs::create_dir_all(&origin).unwrap();
        let init = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&origin)
            .status()
            .unwrap();
        if !init.success() {
            return;
        }
        for (key, value) in [("user.name", "Test"), ("user.email", "test@example.com")] {
            assert!(
                std::process::Command::new("git")
                    .args(["config", key, value])
                    .current_dir(&origin)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        std::fs::write(origin.join("a.rs"), "pub fn a() {}").unwrap();
        for args in [&["add", "a.rs"][..], &["commit", "-qm", "initial"][..]] {
            assert!(
                std::process::Command::new("git")
                    .args(args)
                    .current_dir(&origin)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let wt_a = temp.path().join("worktrees/task-1/attempt-0");
        let wt_b = temp.path().join("worktrees/task-2/attempt-0");
        for (wt, branch) in [(&wt_a, "wt-a"), (&wt_b, "wt-b")] {
            std::fs::create_dir_all(wt.parent().unwrap()).unwrap();
            assert!(
                std::process::Command::new("git")
                    .args(["worktree", "add", "-q", "-b", branch, wt.to_str().unwrap()])
                    .current_dir(&origin)
                    .status()
                    .unwrap()
                    .success()
            );
        }

        let main = workspace_identity(&origin);
        assert_eq!(main.name, "my-repo");
        assert_eq!(main.base, None);

        let a = workspace_identity(&wt_a);
        let b = workspace_identity(&wt_b);
        assert!(a.name.starts_with("my-repo--wt-"), "{}", a.name);
        assert!(b.name.starts_with("my-repo--wt-"), "{}", b.name);
        assert_ne!(a.name, b.name);
        assert_eq!(a.base.as_deref(), Some("my-repo"));
        assert_eq!(workspace_identity(&wt_a), a);
    }

    #[test]
    fn test_partial_sync_does_not_advance_workspace_base() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        clear_sync_cache(root);

        let init = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(root)
            .status()
            .unwrap();
        if !init.success() {
            return;
        }
        for (key, value) in [("user.name", "Test"), ("user.email", "test@example.com")] {
            let configured = std::process::Command::new("git")
                .args(["config", key, value])
                .current_dir(root)
                .status()
                .unwrap();
            assert!(configured.success());
        }

        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.rs"), "pub const A: u8 = 1;").unwrap();
        std::fs::write(root.join("src/b.rs"), "pub const B: u8 = 1;").unwrap();
        for args in [&["add", "src"][..], &["commit", "-qm", "initial"][..]] {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(root)
                .status()
                .unwrap();
            assert!(status.success());
        }
        let initial = prepare_workspace_sync(root, None).unwrap();
        commit_workspace_sync(root, &initial);
        let initial_base = load_sync_cache(root).base_commit_sha;

        std::fs::write(root.join("src/a.rs"), "pub const A: u8 = 2;").unwrap();
        std::fs::write(root.join("src/b.rs"), "pub const B: u8 = 2;").unwrap();
        for args in [&["add", "src"][..], &["commit", "-qm", "both changed"][..]] {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(root)
                .status()
                .unwrap();
            assert!(status.success());
        }

        let partial = prepare_workspace_sync(root, Some(Path::new("src/a.rs"))).unwrap();
        assert_eq!(partial.files.len(), 1);
        commit_workspace_sync(root, &partial);
        assert_eq!(load_sync_cache(root).base_commit_sha, initial_base);

        let remaining = prepare_workspace_sync(root, None).unwrap();
        assert_eq!(remaining.files.len(), 1);
        assert_eq!(remaining.files[0].relative_path, "src/b.rs");
        clear_sync_cache(root);
    }
}

#[cfg(test)]
mod workspace_membership_tests {
    use super::*;

    const ROOT: &str = r#"[workspace]
resolver = "2"
members = [
    "crates/prod-code-protocol",
    "crates/prod-code-mcp",
]
# a fixture is a test bed, not a member
exclude = ["fixtures"]

[workspace.package]
version = "0.2.2"
"#;

    #[test]
    fn the_lists_are_read_across_lines_and_one_liners() {
        let (members, excludes) = workspace_lists(ROOT);
        assert_eq!(
            members,
            ["crates/prod-code-protocol", "crates/prod-code-mcp"]
        );
        assert_eq!(excludes, ["fixtures"]);

        let (members, excludes) =
            workspace_lists("[workspace]\nmembers = [\"a\", \"b/*\"]\nexclude = [\"vendor\"]\n");
        assert_eq!(members, ["a", "b/*"]);
        assert_eq!(excludes, ["vendor"]);
    }

    #[test]
    fn a_member_belongs_to_the_workspace_and_an_excluded_crate_does_not() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::write(root.join("Cargo.toml"), ROOT).expect("root manifest");
        for rel in [
            "crates/prod-code-mcp",
            "fixtures/polyglot-order/core",
            "examples/demo",
        ] {
            std::fs::create_dir_all(root.join(rel)).expect("dirs");
            std::fs::write(
                root.join(rel).join("Cargo.toml"),
                "[package]\nname = \"x\"\n",
            )
            .expect("crate manifest");
        }

        assert!(
            !excluded_from_root_workspace(root, &root.join("crates/prod-code-mcp")),
            "a member belongs to the workspace and must stay with it"
        );
        assert!(
            excluded_from_root_workspace(root, &root.join("fixtures/polyglot-order/core")),
            "a crate under an excluded directory is its own project"
        );
        assert!(
            excluded_from_root_workspace(root, &root.join("examples/demo")),
            "and so is one the members list simply does not mention"
        );
        assert!(
            !excluded_from_root_workspace(root, &root.join("crates")),
            "a directory with no manifest of its own is not a crate at all"
        );
    }

    #[test]
    fn a_glob_member_covers_its_children_and_only_them() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\"]\n",
        )
        .expect("root manifest");
        for rel in ["crates/one", "crates/one/nested", "other"] {
            std::fs::create_dir_all(root.join(rel)).expect("dirs");
            std::fs::write(
                root.join(rel).join("Cargo.toml"),
                "[package]\nname = \"x\"\n",
            )
            .expect("crate manifest");
        }
        assert!(!excluded_from_root_workspace(
            root,
            &root.join("crates/one")
        ));
        assert!(
            excluded_from_root_workspace(root, &root.join("crates/one/nested")),
            "`crates/*` is one level, not a subtree"
        );
        assert!(excluded_from_root_workspace(root, &root.join("other")));
    }

    #[test]
    fn a_plain_package_has_no_workspace_to_belong_to() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"solo\"\n").expect("manifest");
        std::fs::create_dir_all(root.join("sub")).expect("dirs");
        std::fs::write(
            root.join("sub").join("Cargo.toml"),
            "[package]\nname = \"sub\"\n",
        )
        .expect("manifest");
        assert!(
            excluded_from_root_workspace(root, &root.join("sub")),
            "a crate under a plain package answers for itself"
        );
    }
}
