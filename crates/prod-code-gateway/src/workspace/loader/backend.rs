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

pub(crate) async fn load_backend_fallback(
    workspace_root: &Path,
    engine: &str,
) -> Option<Arc<BackendWorker>> {
    BackendWorker::spawn(workspace_root, engine)
        .await
        .ok()
        .map(Arc::new)
}
