/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::sync::atomic::{AtomicU64, Ordering};

use prod_code_protocol::content_hash;

pub async fn write_synced_file(
    target: &std::path::Path,
    content: &[u8],
    executable: bool,
) -> std::io::Result<()> {
    write_synced_file_impl(None, target, content, executable).await
}

pub(crate) async fn write_synced_file_for_workspace(
    workspace_root: &std::path::Path,
    target: &std::path::Path,
    content: &[u8],
    executable: bool,
) -> std::io::Result<()> {
    write_synced_file_impl(Some(workspace_root), target, content, executable).await
}

async fn write_synced_file_impl(
    workspace_root: Option<&std::path::Path>,
    target: &std::path::Path,
    content: &[u8],
    executable: bool,
) -> std::io::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temp = target.with_file_name(format!(
        ".{name}.prod-code-sync-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let coordinated_content = if matches!(
        target.file_name().and_then(|name| name.to_str()),
        Some("tsconfig.json" | "jsconfig.json")
    ) {
        crate::ts_cache::seed::coordinate_tsconfig_content(content)
            .unwrap_or_else(|| content.to_vec())
    } else {
        content.to_vec()
    };
    let written = async {
        tokio::fs::write(&temp, &coordinated_content).await?;
        #[cfg(unix)]
        if executable {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o755)).await?;
        }
        tokio::fs::rename(&temp, target).await
    }
    .await;
    if let Err(error) = written {
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(error);
    }

    if let Some(workspace_root) = workspace_root
        && matches!(
            target.file_name().and_then(|name| name.to_str()),
            Some("tsconfig.json" | "jsconfig.json")
        )
    {
        let metadata_result = if coordinated_content == content {
            crate::sync::config_meta::forget(workspace_root, target).await
        } else {
            crate::sync::config_meta::record(
                workspace_root,
                target,
                content.len() as u64,
                content_hash(content),
                coordinated_content.len() as u64,
                content_hash(&coordinated_content),
            )
            .await
        };
        if let Err(error) = metadata_result {
            tracing::warn!(
                error = %error,
                file = %target.display(),
                "sync metadata update failed after config file was committed"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn write_synced_file_preserves_public_three_argument_api() {
        let temp = tempfile::tempdir().unwrap();
        let root_target = temp.path().join("root.rs");
        crate::write_synced_file(&root_target, b"pub fn root() {}\n", false)
            .await
            .unwrap();
        assert!(root_target.is_file());

        let sync_fs_target = temp.path().join("sync_fs.rs");
        crate::sync_fs::write_synced_file(&sync_fs_target, b"pub fn sync_fs() {}\n", false)
            .await
            .unwrap();
        assert!(sync_fs_target.is_file());
    }
}
