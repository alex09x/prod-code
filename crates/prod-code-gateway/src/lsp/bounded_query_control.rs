/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;
use prod_code_engine_rust::snapshot::SnapshotCancellation;

#[derive(Default)]
pub(super) struct QueryControl {
    state: std::sync::Mutex<(bool, Option<SnapshotCancellation>)>,
}

impl QueryControl {
    pub(super) fn install(
        &self,
        snapshot: &prod_code_engine_rust::RustEngineSnapshot,
    ) -> anyhow::Result<()> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let handle = snapshot.cancellation_handle();
        if state.0 {
            handle.cancel();
            anyhow::bail!("query deadline expired before snapshot execution");
        }
        state.1 = Some(handle);
        Ok(())
    }

    fn cancel(&self) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.0 = true;
        if let Some(handle) = state.1.take() {
            handle.cancel();
            true
        } else {
            false
        }
    }
}

/// Revocation also runs when the request future is dropped by a disconnect/cancellation.
pub(super) struct CancelOnDrop {
    pub(super) control: Arc<QueryControl>,
    engine: Arc<tokio::sync::Mutex<prod_code_engine_rust::RustEngine>>,
    armed: bool,
}

impl CancelOnDrop {
    pub(super) fn new(engine: &Arc<tokio::sync::Mutex<prod_code_engine_rust::RustEngine>>) -> Self {
        Self {
            control: Arc::default(),
            engine: Arc::clone(engine),
            armed: true,
        }
    }

    pub(super) fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if self.armed && self.control.cancel() {
            let engine = Arc::clone(&self.engine);
            // Release idle Salsa handles immediately. Active calls also need Salsa's
            // cancellation flag; never wait for that on a runtime thread. A concurrent
            // writer already cancels readers before taking the VFS lock.
            tokio::task::spawn_blocking(move || {
                if let Ok(mut engine) = engine.try_lock() {
                    engine.trigger_cancellation();
                }
            });
        }
    }
}
