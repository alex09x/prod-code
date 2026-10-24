/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! One-shot, per-workspace barriers at real transaction boundaries. Dropping the installation
//! always releases the worker, including when a test assertion unwinds.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, LazyLock, Mutex};
use std::time::Duration;
use tokio::sync::Semaphore;

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
pub(crate) enum Point {
    PostWrite,
    RestoreDisk,
}

type Hooks = HashMap<(PathBuf, Point), Arc<Pause>>;
static HOOKS: LazyLock<Mutex<Hooks>> = LazyLock::new(Mutex::default);
const BUDGET: Duration = Duration::from_secs(10);

struct Pause {
    entered: Semaphore,
    released: Mutex<bool>,
    changed: Condvar,
    async_release: Semaphore,
}

pub(crate) struct Installation {
    key: (PathBuf, Point),
    pause: Arc<Pause>,
}

impl Installation {
    pub(crate) fn new(root: &Path, point: Point) -> Self {
        let key = (root.to_path_buf(), point);
        let pause = Arc::new(Pause {
            entered: Semaphore::new(0),
            released: Mutex::new(false),
            changed: Condvar::new(),
            async_release: Semaphore::new(0),
        });
        assert!(
            HOOKS
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .insert(key.clone(), pause.clone())
                .is_none()
        );
        Self { key, pause }
    }

    pub(crate) async fn entered(&self) {
        tokio::time::timeout(BUDGET, self.pause.entered.acquire())
            .await
            .expect("transaction worker did not enter barrier")
            .unwrap()
            .forget();
    }

    pub(crate) fn release(&self) {
        *self
            .pause
            .released
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = true;
        self.pause.changed.notify_all();
        self.pause.async_release.close();
    }
}

impl Drop for Installation {
    fn drop(&mut self) {
        self.release();
        HOOKS
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .remove(&self.key);
    }
}

fn take(root: &Path, point: Point) -> Option<Arc<Pause>> {
    HOOKS
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .remove(&(root.to_path_buf(), point))
}

pub(crate) async fn post_write(root: &Path) {
    if let Some(pause) = take(root, Point::PostWrite) {
        pause.entered.add_permits(1);
        assert!(
            tokio::time::timeout(BUDGET, pause.async_release.acquire())
                .await
                .is_ok()
        );
    }
}

pub(crate) fn restore_disk(root: &Path) {
    if let Some(pause) = take(root, Point::RestoreDisk) {
        pause.entered.add_permits(1);
        let (released, result) = pause
            .changed
            .wait_timeout_while(
                pause
                    .released
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner()),
                BUDGET,
                |released| !*released,
            )
            .unwrap_or_else(|poison| poison.into_inner());
        // Timeout reporting must not poison the flag that cleanup uses while unwinding.
        drop(released);
        assert!(
            !result.timed_out(),
            "blocking restore barrier was not released"
        );
    }
}
