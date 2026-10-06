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

/// Largest file whose old bytes a pre-command snapshot keeps, so that the gateway can put it
/// back when the command's changes never reach the client (#262). Source files are far smaller;
/// what is larger is mostly data a command regenerates anyway.
pub const RESTORE_MAX_FILE: u64 = 1024 * 1024;

/// Most bytes one pre-command snapshot keeps in memory. A command runs while its snapshot is
/// held, so a checkout with a large tree of small files must not cost the node gigabytes; a
/// file past the budget is marked stale when it has to be restored, and the client sends it.
pub const RESTORE_BUDGET: u64 = 256 * 1024 * 1024;

/// The old contents of a file, kept to restore it.
pub struct KeptFile {
    pub bytes: Vec<u8>,
    #[cfg(unix)]
    pub mode: u32,
}

/// What a workspace copy held before a remote command ran.
#[derive(Default)]
pub struct TreeSnapshot {
    /// Size and content hash of every file, for detecting what the command changed.
    pub stamps: std::collections::HashMap<String, (u64, u64)>,
    /// The bytes of every file up to [`RESTORE_MAX_FILE`], within [`RESTORE_BUDGET`].
    pub kept: std::collections::HashMap<String, KeptFile>,
}

/// Size and content hash of every file under `root` the sync layer cares about (build output
/// and VCS internals excluded).
pub(crate) fn stamp_tree(root: &std::path::Path) -> std::collections::HashMap<String, (u64, u64)> {
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
pub fn snapshot_tree(root: &std::path::Path) -> TreeSnapshot {
    snapshot_tree_within(root, RESTORE_MAX_FILE, RESTORE_BUDGET)
}

/// [`snapshot_tree`] with its limits given, so that a test can exceed them cheaply.
pub(crate) fn snapshot_tree_within(root: &std::path::Path, max_file: u64, mut budget: u64) -> TreeSnapshot {
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
pub struct Restored {
    /// Every file put back as it was, created by the command and removed, or removed because
    /// its old bytes were not kept: what a warm engine must be told.
    pub files: Vec<FileDelta>,
    /// The files among them that could not be put back and are gone from the copy.
    pub stale: Vec<String>,
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
pub(crate) fn restore_tree(
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
pub async fn restore_after_lost_client(
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
pub fn changed_since(
    root: &std::path::Path,
    before: &std::collections::HashMap<String, (u64, u64)>,
) -> Vec<FileDelta> {
    pub(crate) const MAX_PULL_FILE: u64 = 5 * 1024 * 1024;
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



/// Sends files changed by a remote execution back to the client, or restores the pre-command
/// snapshot if the client disconnected before receiving them.
pub async fn send_exec_changes_and_recover(
    workspace_manager: &WorkspaceManager,
    workspace: &Path,
    workspace_str: &str,
    before: Arc<TreeSnapshot>,
    snapshot_started: Instant,
    framed: &mut Framed<AnyStream, ProdCodeCodec>,
) -> Result<Vec<FileDelta>> {
    let (root, snapshot) = (workspace.to_path_buf(), Arc::clone(&before));
    let files = tokio::task::spawn_blocking(move || changed_since(&root, &snapshot.stamps))
        .await
        .unwrap_or_default();
    if !files.is_empty() {
        tracing::info!(
            workspace = %workspace_str,
            files = files.len(),
            "🛠️ [EXEC] sending back files the command changed"
        );
        if let Err(_e) = framed
            .send(WireMessage::ExecChanges(ExecChanges {
                files: files.clone(),
            }))
            .await
        {
            let restored = restore_after_lost_client(
                workspace_manager,
                workspace,
                before,
                snapshot_started,
            )
            .await;
            tracing::info!(
                workspace = %workspace_str,
                "🛠️ [EXEC] client left before the changes were sent; {restored} file(s) the command changed restored"
            );
        }
    }
    Ok(files)
}
