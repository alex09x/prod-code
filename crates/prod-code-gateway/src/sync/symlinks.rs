/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use prod_code_protocol::messages::sync::SyncSymlink;
use serde::{Deserialize, Serialize};

const MANIFEST: &str = ".prod/tracked-symlinks.json";
static TEMP_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Default, Serialize, Deserialize)]
struct ManagedSymlinks {
    entries: BTreeMap<String, String>,
}

#[derive(Debug, Default)]
pub(crate) struct ApplyResult {
    pub updated: usize,
    pub deleted: usize,
    pub failed: Vec<String>,
}

pub(crate) fn apply(root: &Path, requested: &[SyncSymlink]) -> ApplyResult {
    let mut result = ApplyResult::default();
    let Ok(canonical_root) = std::fs::canonicalize(root) else {
        result.failed.push(MANIFEST.to_string());
        return result;
    };
    let manifest_path = canonical_root.join(MANIFEST);
    let previous = match read_manifest(&manifest_path) {
        Ok(previous) => previous,
        Err(()) => {
            result.failed.push(MANIFEST.to_string());
            return result;
        }
    };
    let mut wanted = BTreeMap::new();
    for entry in requested {
        if wanted
            .insert(entry.relative_path.clone(), entry.target.clone())
            .is_some()
            || safe_link_path(&canonical_root, &entry.relative_path).is_none()
            || !target_is_contained(&canonical_root, &entry.relative_path, &entry.target)
        {
            result.failed.push(entry.relative_path.clone());
        }
    }
    if !result.failed.is_empty() {
        result.failed.sort();
        result.failed.dedup();
        return result;
    }

    for (relative, old_target) in &previous.entries {
        if wanted.contains_key(relative) {
            continue;
        }
        let Some(path) = safe_link_path(&canonical_root, relative) else {
            result.failed.push(relative.clone());
            continue;
        };
        match std::fs::read_link(&path) {
            Ok(target) if target.to_str() == Some(old_target) => {
                if std::fs::remove_file(&path).is_ok() {
                    result.deleted += 1;
                } else {
                    result.failed.push(relative.clone());
                }
            }
            Ok(_) | Err(_) => {}
        }
    }

    let mut retained = BTreeMap::new();
    for (relative, target) in wanted {
        let Some(path) = safe_link_path(&canonical_root, &relative) else {
            result.failed.push(relative.clone());
            continue;
        };
        let current = std::fs::read_link(&path).ok();
        if current.as_deref().and_then(|p| p.to_str()) == Some(target.as_str()) {
            retained.insert(relative, target);
            continue;
        }
        if let Some(parent) = path.parent()
            && (std::fs::create_dir_all(parent).is_err()
                || safe_link_path(&canonical_root, &relative).is_none())
        {
            result.failed.push(relative.clone());
            continue;
        }
        if std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
        {
            result.failed.push(relative.clone());
            continue;
        }
        let Some(parent) = path.parent() else {
            result.failed.push(relative.clone());
            continue;
        };
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            result.failed.push(relative.clone());
            continue;
        };
        let temp = parent.join(format!(
            ".{name}.prod-link-{}",
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        if create_symlink(Path::new(&target), &temp).is_err() {
            result.failed.push(relative.clone());
            continue;
        }
        if std::fs::rename(&temp, &path).is_err() {
            let _ = std::fs::remove_file(&temp);
            result.failed.push(relative.clone());
            continue;
        }
        result.updated += 1;
        retained.insert(relative, target);
    }

    // Keep failed old entries managed so a later full sync can safely retry their removal.
    for (relative, target) in previous.entries {
        if result.failed.contains(&relative) && !retained.contains_key(&relative) {
            retained.insert(relative, target);
        }
    }
    if write_manifest(&canonical_root, &retained).is_err() {
        result.failed.push(MANIFEST.to_string());
        result.failed.extend(retained.keys().cloned());
    }
    result.failed.sort();
    result.failed.dedup();
    result
}

fn safe_link_path(root: &Path, relative: &str) -> Option<PathBuf> {
    if relative.is_empty() || relative.contains('\\') || Path::new(relative).is_absolute() {
        return None;
    }
    let components: Vec<_> = Path::new(relative).components().collect();
    if components.is_empty()
        || components
            .iter()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return None;
    }
    let mut parent_path = root.to_path_buf();
    for component in components.iter().take(components.len() - 1) {
        parent_path.push(component.as_os_str());
        match std::fs::symlink_metadata(&parent_path) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => return None,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(_) => return None,
        }
    }
    Some(root.join(relative))
}

fn target_is_contained(root: &Path, relative: &str, target: &str) -> bool {
    let target_path = Path::new(target);
    if target.is_empty() || target_path.is_absolute() {
        return false;
    }
    let mut normalized: Vec<std::ffi::OsString> = Path::new(relative)
        .parent()
        .into_iter()
        .flat_map(Path::components)
        .filter_map(|c| match c {
            Component::Normal(name) => Some(name.to_os_string()),
            _ => None,
        })
        .collect();
    for component in target_path.components() {
        match component {
            Component::Normal(name) => normalized.push(name.to_os_string()),
            Component::CurDir => {}
            Component::ParentDir if normalized.pop().is_some() => {}
            _ => return false,
        }
    }
    let mut current = root.to_path_buf();
    for (index, component) in normalized.iter().enumerate() {
        current.push(component);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                if !std::fs::canonicalize(&current).is_ok_and(|p| p.starts_with(root)) {
                    return false;
                }
            }
            Ok(metadata) if index + 1 < normalized.len() && !metadata.is_dir() => return false,
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(_) => return false,
        }
    }
    true
}

fn read_manifest(path: &Path) -> Result<ManagedSymlinks, ()> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| ()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(ManagedSymlinks::default())
        }
        Err(_) => Err(()),
    }
}

fn write_manifest(root: &Path, entries: &BTreeMap<String, String>) -> std::io::Result<()> {
    let dir = root.join(".prod");
    match std::fs::symlink_metadata(&dir) {
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {}
        Ok(_) => return Err(std::io::Error::other("unsafe symlink manifest directory")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => std::fs::create_dir(&dir)?,
        Err(error) => return Err(error),
    }
    let bytes = serde_json::to_vec(&ManagedSymlinks {
        entries: entries.clone(),
    })
    .map_err(std::io::Error::other)?;
    let path = root.join(MANIFEST);
    if std::fs::read(&path).is_ok_and(|current| current == bytes) {
        return Ok(());
    }
    let temporary = dir.join(format!(
        "tracked-symlinks-{}.tmp",
        TEMP_ID.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&temporary, bytes)?;
    if let Err(error) = std::fs::rename(&temporary, path) {
        let _ = std::fs::remove_file(temporary);
        return Err(error);
    }
    Ok(())
}

#[cfg(unix)]
fn create_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(not(unix))]
fn create_symlink(_: &Path, _: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "symlink sync requires a Unix gateway",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use prod_code_protocol::SyncRequest;

    #[cfg(unix)]
    #[test]
    fn writes_dangling_tracked_link_and_rejects_workspace_escape() {
        let root = tempfile::tempdir().expect("workspace");
        let entry = SyncSymlink {
            relative_path: "swift/Sources/App/_bindings".into(),
            target: "../../../target/bindings".into(),
        };
        let result = apply(root.path(), std::slice::from_ref(&entry));
        assert!(result.failed.is_empty(), "{:?}", result.failed);
        assert_eq!(result.updated, 1);
        let path = root.path().join(&entry.relative_path);
        assert_eq!(std::fs::read_link(&path).unwrap(), Path::new(&entry.target));

        std::fs::create_dir_all(root.path().join("target/bindings")).unwrap();
        std::fs::write(
            root.path().join("target/bindings/generated.swift"),
            "struct Generated {}",
        )
        .unwrap();
        assert!(path.exists());
        assert_eq!(
            std::fs::read_to_string(path.join("generated.swift")).unwrap(),
            "struct Generated {}"
        );

        let removed = apply(root.path(), &[]);
        assert_eq!(removed.deleted, 1);
        assert!(path.symlink_metadata().is_err());

        let escaped = SyncSymlink {
            relative_path: "swift/Sources/App/_outside".into(),
            target: "../../../../outside".into(),
        };
        let rejected = apply(root.path(), &[entry, escaped]);
        assert!(rejected.failed.iter().any(|p| p.ends_with("_outside")));
        assert!(!root.path().join("swift/Sources/App/_outside").exists());
    }

    #[tokio::test]
    async fn sync_response_acknowledges_a_symlink_manifest() {
        let storage = tempfile::tempdir().expect("gateway storage");
        let manager = crate::workspace::WorkspaceManager::new();
        let response = crate::sync::apply_sync(
            storage.path(),
            &manager,
            SyncRequest {
                client_workspace_root: "/client/symlink-ack".into(),
                files: Vec::new(),
                symlinks: Some(Vec::new()),
                clean_others: false,
                base_workspace_name: Some("symlink-ack".into()),
            },
        )
        .await;
        assert!(
            response
                .stale_paths
                .iter()
                .any(|path| path == prod_code_protocol::messages::sync::SYNC_SYMLINKS_ACK)
        );
    }
}
