/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Error, Result};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::backend::BackendWorker;
use crate::workspace::types::RustLoader;

pub(crate) async fn load_rust(
    workspace_root: &Path,
    rust_loader: &RustLoader,
) -> Result<(
    Option<Arc<Mutex<prod_code_engine_rust::RustEngine>>>,
    Option<Arc<BackendWorker>>,
)> {
    let ws_path = workspace_root.to_path_buf();
    let load = Arc::clone(rust_loader);
    let loaded_engine = match tokio::task::spawn_blocking(move || {
        // Every worktree copy builds into its own target directory: worktrees
        // never share cargo state or wait on each other's build lock.
        load(&ws_path)
    })
    .await
    {
        Ok(Ok(engine)) => {
            tracing::info!(workspace = ?workspace_root, "In-memory RustEngine (ra_ap_ide) loaded into RAM");
            Some(Arc::new(Mutex::new(engine)))
        }
        Ok(Err(err)) => engine_load_error(err)?.map(|engine| Arc::new(Mutex::new(engine))),
        Err(err) => {
            tracing::warn!(error = %err, "Failed to load in-memory RustEngine; falling back to subprocess");
            None
        }
    };

    if let Some(re) = loaded_engine {
        Ok((Some(re), None))
    } else {
        let backend = BackendWorker::spawn(workspace_root, "rust")
            .await
            .ok()
            .map(Arc::new);
        Ok((None, backend))
    }
}

fn engine_load_error(err: Error) -> Result<Option<prod_code_engine_rust::RustEngine>> {
    if err
        .downcast_ref::<prod_code_engine_rust::engine::load::ProcMacroCapacityError>()
        .is_some()
    {
        return Err(err);
    }
    tracing::warn!(error = %err, "Failed to load in-memory RustEngine; falling back to subprocess");
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::engine_load_error;
    use prod_code_engine_rust::engine::load::ProcMacroCapacityError;

    #[test]
    fn proc_macro_capacity_error_propagates_without_fallback() {
        let result = engine_load_error(anyhow::Error::new(ProcMacroCapacityError::new(8)));

        let err = match result {
            Err(err) => err,
            Ok(_) => panic!("capacity exhaustion must propagate instead of falling back"),
        };
        assert!(err.downcast_ref::<ProcMacroCapacityError>().is_some());
    }

    #[test]
    fn proc_macro_capacity_classifier_keeps_generic_fallback() {
        let result = engine_load_error(anyhow::anyhow!("ordinary load failure"));

        assert!(result.unwrap().is_none());
    }
}
