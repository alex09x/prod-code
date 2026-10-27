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
                } else {
                    // The node says the file is not executable: a local copy that was keeps its
                    // other permission bits but loses the executable ones (#1477).
                    use std::os::unix::fs::PermissionsExt;
                    if let Ok(metadata) = target.metadata() {
                        let mode = metadata.permissions().mode();
                        if mode & 0o111 != 0 {
                            let _ = std::fs::set_permissions(
                                &target,
                                std::fs::Permissions::from_mode(mode & !0o111),
                            );
                        }
                    }
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

#[cfg(all(test, unix))]
mod tests {
    use super::apply_pulled_files_for;
    use prod_code_protocol::FileDelta;
    use std::os::unix::fs::PermissionsExt;

    fn delta(path: &str, text: &str, is_executable: bool) -> FileDelta {
        FileDelta {
            relative_path: path.to_string(),
            content: Some(text.as_bytes().to_vec()),
            is_executable,
        }
    }

    fn mode(path: &std::path::Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    /// The node's executable bit wins over the local file's, both ways, and only the
    /// executable bits change (#1477).
    #[test]
    fn a_pulled_file_takes_the_nodes_executable_bit() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let was_executable = root.join("run.sh");
        std::fs::write(&was_executable, "#!/bin/sh\necho old\n").unwrap();
        std::fs::set_permissions(&was_executable, std::fs::Permissions::from_mode(0o750)).unwrap();
        let plain = root.join("notes.txt");
        std::fs::write(&plain, "old\n").unwrap();
        std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o640)).unwrap();

        apply_pulled_files_for(
            root,
            "test-node",
            &[
                delta("run.sh", "#!/bin/sh\necho new\n", false),
                delta("notes.txt", "new\n", true),
                delta("new.txt", "fresh\n", false),
            ],
        )
        .unwrap();
        assert_eq!(
            mode(&was_executable),
            0o640,
            "a shebang does not keep it executable"
        );
        assert_eq!(mode(&plain) & 0o111, 0o111);
        assert_eq!(mode(&root.join("new.txt")) & 0o111, 0, "a new plain file");
        assert_eq!(
            std::fs::read_to_string(&was_executable).unwrap(),
            "#!/bin/sh\necho new\n"
        );
    }
}
