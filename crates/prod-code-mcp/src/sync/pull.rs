/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::sync::cache::{load_sync_cache_for, save_sync_cache_for};
use crate::sync::entry::sync_file_entry;
use anyhow::Result;
use prod_code_protocol::FileDelta;
use std::path::{Path, PathBuf};

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
        crate::call_tree::clear_call_hierarchy_cache_for(&canonical_root);
    }
    Ok(touched)
}

/// Pulls files from the last execution node, falling back to `remote` before the first exec.
pub async fn pull_remote_files(
    remote: std::net::SocketAddr,
    root: &Path,
    files: &[PathBuf],
) -> Result<Vec<String>> {
    let node = crate::exec::last_exec_node(root).unwrap_or(remote);
    Ok(super::artifact::pull_remote_files_from(node, root, files)
        .await?
        .files)
}
