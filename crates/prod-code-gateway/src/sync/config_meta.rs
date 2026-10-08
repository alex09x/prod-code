/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::{Component, Path, PathBuf};

pub(crate) const METADATA_DIR: &str = ".prod-code-sync-meta";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ConfigStamp {
    client_size: u64,
    client_hash: u64,
    server_size: u64,
    server_hash: u64,
}

fn metadata_path(workspace_root: &Path, config_path: &Path) -> Option<PathBuf> {
    let relative = config_path.strip_prefix(workspace_root).ok()?;
    if !relative
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
        || !matches!(
            config_path.file_name()?.to_str()?,
            "tsconfig.json" | "jsconfig.json"
        )
    {
        return None;
    }
    let workspace_name = workspace_root.file_name()?;
    let storage_root = workspace_root.parent()?;
    let mut path = storage_root
        .join(METADATA_DIR)
        .join(workspace_name)
        .join(relative);
    path.set_extension("prod-code-sync-stamp");
    Some(path)
}

fn workspace_metadata_path(workspace_root: &Path) -> Option<PathBuf> {
    Some(
        workspace_root
            .parent()?
            .join(METADATA_DIR)
            .join(workspace_root.file_name()?),
    )
}

pub(crate) fn matches(
    workspace_root: &Path,
    config_path: &Path,
    client_size: u64,
    client_hash: u64,
    server_size: u64,
    server_hash: u64,
) -> bool {
    let Some(path) = metadata_path(workspace_root, config_path) else {
        return false;
    };
    let Ok(content) = std::fs::read_to_string(path) else {
        return false;
    };
    let values = content
        .lines()
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>();
    let Ok(values) = values else {
        return false;
    };
    if values.len() != 4 {
        return false;
    }
    let saved = ConfigStamp {
        client_size: values[0],
        client_hash: values[1],
        server_size: values[2],
        server_hash: values[3],
    };
    saved
        == (ConfigStamp {
            client_size,
            client_hash,
            server_size,
            server_hash,
        })
}

pub(crate) async fn record(
    workspace_root: &Path,
    config_path: &Path,
    client_size: u64,
    client_hash: u64,
    server_size: u64,
    server_hash: u64,
) -> std::io::Result<()> {
    let Some(path) = metadata_path(workspace_root, config_path) else {
        return Ok(());
    };
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "metadata path has no parent",
        )
    })?;
    tokio::fs::create_dir_all(parent).await?;
    tokio::fs::write(
        path,
        format!("{client_size}\n{client_hash}\n{server_size}\n{server_hash}\n"),
    )
    .await
}

pub(crate) async fn forget(workspace_root: &Path, config_path: &Path) -> std::io::Result<()> {
    let Some(path) = metadata_path(workspace_root, config_path) else {
        return Ok(());
    };
    remove_async(path).await
}

pub(crate) fn forget_sync(workspace_root: &Path, config_path: &Path) -> std::io::Result<()> {
    let Some(path) = metadata_path(workspace_root, config_path) else {
        return Ok(());
    };
    remove_sync(path)
}

async fn remove_async(path: PathBuf) -> std::io::Result<()> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn remove_sync(path: PathBuf) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

pub(crate) fn remove_workspace(workspace_root: &Path) {
    if let Some(path) = workspace_metadata_path(workspace_root) {
        let _ = std::fs::remove_dir_all(path);
    }
}

pub(crate) fn copy_workspace(source_root: &Path, target_root: &Path) -> std::io::Result<()> {
    let Some(source) = workspace_metadata_path(source_root) else {
        return Ok(());
    };
    let Some(target) = workspace_metadata_path(target_root) else {
        return Ok(());
    };
    match std::fs::remove_dir_all(&target) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    if !source.is_dir() {
        return Ok(());
    }
    copy_metadata_tree(&source, &target)
}

fn copy_metadata_tree(source: &Path, target: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(target)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        if file_type.is_dir() {
            copy_metadata_tree(&source_path, &target_path)?;
        } else if file_type.is_file() {
            std::fs::copy(source_path, target_path)?;
        }
    }
    Ok(())
}

pub(crate) fn prune_orphans(storage_root: &Path) {
    let metadata_root = storage_root.join(METADATA_DIR);
    let Ok(entries) = std::fs::read_dir(&metadata_root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !storage_root.join(entry.file_name()).is_dir() {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{METADATA_DIR, copy_workspace, matches, prune_orphans, record, remove_workspace};

    #[tokio::test]
    async fn typescript_sync_metadata_tracks_client_content_and_prunes_orphans() {
        let storage = tempfile::tempdir().unwrap();
        let workspace = storage.path().join("typescript-repo");
        std::fs::create_dir_all(&workspace).unwrap();
        let config = workspace.join("tsconfig.json");
        let metadata_dir = storage.path().join(METADATA_DIR).join("typescript-repo");
        let metadata_file = metadata_dir.join("tsconfig.prod-code-sync-stamp");

        record(&workspace, &config, 10, 11, 12, 13).await.unwrap();
        assert!(matches(&workspace, &config, 10, 11, 12, 13));
        assert!(!matches(&workspace, &config, 10, 99, 12, 13));

        let seeded_workspace = storage.path().join("typescript-repo--wt-0001");
        std::fs::create_dir_all(&seeded_workspace).unwrap();
        copy_workspace(&workspace, &seeded_workspace).unwrap();
        assert!(matches(
            &seeded_workspace,
            &seeded_workspace.join("tsconfig.json"),
            10,
            11,
            12,
            13
        ));

        prune_orphans(storage.path());
        assert!(metadata_file.is_file());
        remove_workspace(&workspace);
        assert!(!metadata_dir.exists());

        let orphan_dir = storage.path().join(METADATA_DIR).join("orphan");
        std::fs::create_dir_all(&orphan_dir).unwrap();
        std::fs::write(orphan_dir.join("stale.stamp"), b"stale").unwrap();
        prune_orphans(storage.path());
        assert!(!orphan_dir.exists());
    }
}
