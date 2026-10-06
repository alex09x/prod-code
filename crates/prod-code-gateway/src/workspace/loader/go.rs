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

use crate::backend::BackendWorker;

pub(crate) async fn load_go(
    workspace_root: &Path,
) -> (
    Option<Arc<prod_code_engine_go::GoEngine>>,
    Option<Arc<BackendWorker>>,
) {
    match prod_code_engine_go::GoEngine::load(
        workspace_root,
        prod_code_engine_go::GoConfig::default(),
    )
    .await
    {
        Ok(ge) => {
            tracing::info!(workspace = ?workspace_root, "Supervised GoEngine (gopls) active");
            (Some(Arc::new(ge)), None)
        }
        Err(err) => {
            tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn GoEngine; falling back to subprocess");
            let backend = BackendWorker::spawn(workspace_root, "go")
                .await
                .ok()
                .map(Arc::new);
            (None, backend)
        }
    }
}
