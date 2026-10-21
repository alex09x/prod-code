/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path};

use prod_code_protocol::{PULL_CHUNK_BYTES, PullFileRequest, PullFileResponse, ReadFileResponse};

/// Resolve exactly the same copy as exec, without creating a copy or loading an engine.
pub fn pull_workspace_file(storage_root: &Path, req: &PullFileRequest) -> PullFileResponse {
    let workspace = crate::workspace::server_workspace_path(
        storage_root,
        &req.client_workspace_root,
        req.base_workspace_name.as_deref(),
    );
    let mut response = PullFileResponse {
        server_workspace_root: workspace.to_string_lossy().into_owned(),
        offset: req.offset,
        total_bytes: 0,
        file: ReadFileResponse {
            path: req.path.clone(),
            content: None,
            truncated: false,
            is_executable: None,
            error: None,
        },
    };
    let read = || -> anyhow::Result<(Vec<u8>, u64, bool)> {
        let relative = Path::new(&req.path);
        anyhow::ensure!(
            !relative.as_os_str().is_empty()
                && relative
                    .components()
                    .all(|c| matches!(c, Component::Normal(_))),
            "refused: path must belong to this checkout's copy"
        );
        // Reject symlinked copies as well as files linking outside this copy. In particular,
        // the storage directory as a whole is never an artifact read permission.
        let storage = std::fs::canonicalize(storage_root)?;
        let copy = std::fs::canonicalize(&workspace)?;
        anyhow::ensure!(
            copy == workspace && copy.starts_with(&storage),
            "refused: checkout copy escapes its storage location"
        );
        let (read_root, relative) = super::artifact_target::resolve(&copy, relative)?;
        let candidate = read_root.join(relative);
        // Validate existing ancestors before reporting a missing file, so a dangling path
        // through a symlink into another copy is still refused.
        for ancestor in candidate.ancestors() {
            match std::fs::canonicalize(ancestor) {
                Ok(resolved) => {
                    anyhow::ensure!(
                        resolved.starts_with(&read_root),
                        "refused: path is outside this checkout's copy"
                    );
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            }
        }
        let canonical = std::fs::canonicalize(&candidate)?;
        anyhow::ensure!(
            canonical.starts_with(&read_root),
            "refused: path is outside this checkout's copy"
        );
        let mut file = std::fs::File::open(canonical)?;
        let metadata = file.metadata()?;
        anyhow::ensure!(
            metadata.is_file(),
            "refused: artifact is not a regular file"
        );
        anyhow::ensure!(
            req.offset <= metadata.len(),
            "artifact changed during pull: offset exceeds file size"
        );
        file.seek(SeekFrom::Start(req.offset))?;
        let mut bytes = Vec::new();
        file.take(PULL_CHUNK_BYTES).read_to_end(&mut bytes)?;
        #[cfg(unix)]
        let executable = {
            use std::os::unix::fs::PermissionsExt;
            metadata.permissions().mode() & 0o111 != 0
        };
        #[cfg(not(unix))]
        let executable = false;
        Ok((bytes, metadata.len(), executable))
    };
    match read() {
        Ok((bytes, total, executable)) => {
            response.file.truncated = req.offset + (bytes.len() as u64) < total;
            response.file.content = Some(bytes);
            response.file.is_executable = Some(executable);
            response.total_bytes = total;
        }
        Err(error) => {
            let detail = if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound)
            {
                "file not found in this checkout's copy".to_string()
            } else {
                error.to_string()
            };
            response.file.error = Some(format!(
                "cannot pull {} from copy {}: {detail}",
                req.path, response.server_workspace_root
            ));
        }
    }
    response
}

#[cfg(test)]
mod tests;
