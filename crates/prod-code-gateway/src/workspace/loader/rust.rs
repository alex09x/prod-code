/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::backend::BackendWorker;
use crate::workspace::types::RustLoader;

pub(crate) async fn load_rust(
    workspace_root: &Path,
    rust_loader: &RustLoader,
) -> (
    Option<Arc<Mutex<prod_code_engine_rust::RustEngine>>>,
    Option<Arc<BackendWorker>>,
) {
    let ws_path = workspace_root.to_path_buf();
    let load = Arc::clone(rust_loader);
    let loaded_engine = tokio::task::spawn_blocking(move || {
        // Every worktree copy builds into its own target directory: worktrees
        // never share cargo state or wait on each other's build lock.
        load(&ws_path)
    })
    .await
    .ok()
    .and_then(|res| match res {
        Ok(e) => {
            tracing::info!(workspace = ?workspace_root, "In-memory RustEngine (ra_ap_ide) loaded into RAM");
            Some(Arc::new(Mutex::new(e)))
        }
        Err(err) => {
            tracing::warn!(error = %err, "Failed to load in-memory RustEngine; falling back to subprocess");
            None
        }
    });

    if let Some(re) = loaded_engine {
        (Some(re), None)
    } else {
        let backend = BackendWorker::spawn(workspace_root, "rust")
            .await
            .ok()
            .map(Arc::new);
        (None, backend)
    }
}
