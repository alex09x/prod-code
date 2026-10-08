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
    workspace_root: &std::path::Path,
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

    if matches!(
        target.file_name().and_then(|name| name.to_str()),
        Some("tsconfig.json" | "jsconfig.json")
    ) {
        let metadata = crate::sync::config_meta::record(
            workspace_root,
            target,
            content.len() as u64,
            content_hash(content),
            coordinated_content.len() as u64,
            content_hash(&coordinated_content),
        );
        if coordinated_content == content {
            crate::sync::config_meta::forget(workspace_root, target).await?;
        } else {
            metadata.await?;
        }
    }
    Ok(())
}
