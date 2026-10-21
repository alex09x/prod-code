/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::{Path, PathBuf};

fn metadata_path(workspace: &Path) -> Option<PathBuf> {
    Some(
        workspace
            .parent()?
            .join(".prod-code-artifact-meta")
            .join(workspace.file_name()?)
            .join("target.json"),
    )
}

// Match the isolated namespace created by resolve_ram_build_cache. A recorded cache is
// permission to read only that copy's target directory, never the cache base as a whole.
fn owns_target(workspace: &Path, target: &Path) -> bool {
    let Some(namespace) = crate::runner::ram_cache::ram_cache_namespace(workspace) else {
        return false;
    };
    target.file_name().is_some_and(|s| s == "target")
        && target
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|s| s == namespace.as_str())
}

/// Persist execution's actual cache selection outside the client-synced tree. Reading it
/// never allocates a cache or reevaluates RAM headroom, which may have changed since exec.
pub(crate) fn record(workspace: &Path, target: Option<&Path>, env: &[(String, String)]) {
    let Some(path) = metadata_path(workspace) else {
        return;
    };
    let target = target
        .filter(|_| !env.iter().any(|(key, _)| key == "CARGO_TARGET_DIR"))
        .and_then(|p| std::fs::canonicalize(p).ok())
        .filter(|p| owns_target(workspace, p));
    let result = (|| -> anyhow::Result<()> {
        std::fs::create_dir_all(path.parent().unwrap())?;
        let bytes = serde_json::to_vec(&target)?;
        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let temporary = path.with_extension(format!("tmp-{}-{sequence}", std::process::id()));
        std::fs::write(&temporary, bytes)?;
        if let Err(error) = std::fs::rename(&temporary, &path) {
            let _ = std::fs::remove_file(&temporary);
            return Err(error.into());
        }
        Ok(())
    })();
    if let Err(error) = result {
        tracing::warn!(%error, "could not record artifact target for exec");
    }
}

/// Map the conventional target path to the last execution's isolated build directory.
pub(crate) fn resolve(workspace: &Path, relative: &Path) -> anyhow::Result<(PathBuf, PathBuf)> {
    let Some(path) = metadata_path(workspace) else {
        return Ok((workspace.into(), relative.into()));
    };
    let suffix = match relative.strip_prefix("target") {
        Ok(suffix) => suffix,
        Err(_) => return Ok((workspace.into(), relative.into())),
    };
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((workspace.into(), relative.into()));
        }
        Err(error) => return Err(error.into()),
    };
    let target: Option<PathBuf> = serde_json::from_slice(&bytes)?;
    let Some(target) = target else {
        return Ok((workspace.into(), relative.into()));
    };
    anyhow::ensure!(
        owns_target(workspace, &target),
        "refused: build target belongs to another checkout"
    );
    let canonical = std::fs::canonicalize(&target)?;
    anyhow::ensure!(
        canonical == target,
        "refused: build target is a symlink outside its recorded location"
    );
    Ok((canonical, suffix.into()))
}
