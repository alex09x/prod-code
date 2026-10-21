/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Hands synced files to the loaded Rust engines without making the sync answer wait for them.
//!
//! A Salsa write waits until every outstanding analysis snapshot is dropped, and a query stuck
//! on a snapshot kept the engine lock held for good. A sync that awaited that lock never
//! answered, and held the workspace's sync lock while it waited, so the next probe hung too
//! (#1143). The files are on disk before this runs; the engines take them when they are free.

use crate::*;

/// How long a sync answer waits for the engines to take the new base before it is sent anyway.
pub(crate) const BASE_UPDATE_WAIT: Duration = Duration::from_secs(5);

type EngineLock = Arc<tokio::sync::Mutex<prod_code_engine_rust::RustEngine>>;

/// Applies the on-disk state of `paths` as the new base of every engine in `engines`, waiting
/// at most `wait`. Returns whether every engine took the update in time; one that did not
/// takes it in the background as soon as its lock is free.
///
/// Each engine reads a file's text from disk under its own lock rather than carrying the text
/// of this sync, so a deferred update that runs after a newer sync applies the newer content.
pub(crate) async fn push_base_updates(
    engines: Vec<EngineLock>,
    paths: Vec<std::path::PathBuf>,
    wait: Duration,
) -> bool {
    if engines.is_empty() || paths.is_empty() {
        return true;
    }
    let paths = Arc::new(paths);
    let task = tokio::spawn(async move {
        for engine in engines {
            let guard = engine.lock_owned().await;
            let paths = Arc::clone(&paths);
            let _ = tokio::task::spawn_blocking(move || {
                let mut engine = guard;
                for path in paths.iter() {
                    apply_one(&mut engine, path);
                }
            })
            .await;
        }
    });
    match tokio::time::timeout(wait, task).await {
        Ok(_) => true,
        Err(_) => {
            tracing::warn!(
                wait_secs = wait.as_secs(),
                "Rust engine busy; the synced files reach the analyzer once it is free"
            );
            false
        }
    }
}

fn apply_one(engine: &mut prod_code_engine_rust::RustEngine, path: &std::path::Path) {
    let text = match std::fs::read(path) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => Some(text),
            Err(_) => return,
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            tracing::warn!(error = %e, file = %path.display(), "synced file unreadable for the analyzer");
            return;
        }
    };
    let removal = text.is_none();
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        engine.update_base(path, text)
    }));
    match res {
        Ok(Err(e)) if removal => {
            tracing::warn!(error = %e, file = %path.display(), "base removal failed");
        }
        Ok(Err(e)) => {
            tracing::warn!(error = %e, file = %path.display(), "base update failed");
        }
        Err(_) => {
            tracing::warn!(file = %path.display(), "base update panicked; continuing");
        }
        Ok(Ok(())) => {}
    }
}
