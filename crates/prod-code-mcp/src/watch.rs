/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Change tracking for long-lived client processes (the MCP server): a recursive filesystem
//! watcher per workspace root bumps a generation counter on every event, so a query only pays
//! the pre-flight sync (three git subprocesses, ~100 ms on a large tree) when the tree actually
//! changed since the last successful sync. A safety interval forces a sync anyway, in case the
//! watcher dropped events.

use crate::config::{IgnoreSnapshot, is_config_or_ignore_file, load_config};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

mod lifecycle;
pub(crate) use lifecycle::{Lease, RootIdentity, lease, root_identity};

/// Force a sync at least this often even without observed changes.
pub const MAX_SYNC_AGE: Duration = Duration::from_secs(60);

struct WatchState {
    root: PathBuf,
    generation: Arc<AtomicU64>,
    filter_dirty: Arc<AtomicBool>,
    snapshot: RwLock<Arc<IgnoreSnapshot>>,
}

struct Tracked {
    state: Arc<WatchState>,
    synced: Mutex<(u64, Option<Instant>)>,
    _watcher: Option<RecommendedWatcher>,
}

fn next_generation() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

impl WatchState {
    fn changed(&self) {
        // fetch_max prevents concurrent callbacks from publishing tokens out of order.
        self.generation
            .fetch_max(next_generation(), Ordering::Relaxed);
    }
}

impl Tracked {
    fn new(root: &Path) -> Self {
        let config = load_config(root);
        let state = Arc::new(WatchState {
            root: root.to_path_buf(),
            generation: Arc::new(AtomicU64::new(next_generation())),
            filter_dirty: Arc::new(AtomicBool::new(false)),
            snapshot: RwLock::new(Arc::new(IgnoreSnapshot::build(root, &config.watch))),
        });
        let watcher = start_watcher(Arc::clone(&state));
        Self {
            state,
            synced: Mutex::new((0, None)),
            _watcher: watcher,
        }
    }
}

fn start_watcher(state: Arc<WatchState>) -> Option<RecommendedWatcher> {
    let root = state.root.clone();
    let state_for_watcher = Arc::clone(&state);
    let mut watcher =
        notify::recommended_watcher(move |event: notify::Result<notify::Event>| match event {
            Ok(event) => {
                if event.paths.is_empty() {
                    state_for_watcher.changed();
                    return;
                }
                let snapshot = {
                    let guard = state_for_watcher
                        .snapshot
                        .read()
                        .unwrap_or_else(|e| e.into_inner());
                    Arc::clone(&*guard)
                };
                let mut should_bump = false;
                for path in &event.paths {
                    if let Ok(rel) = path.strip_prefix(&root)
                        && is_config_or_ignore_file(rel)
                    {
                        state_for_watcher
                            .filter_dirty
                            .store(true, Ordering::Release);
                        should_bump = true;
                        break;
                    }
                    if !snapshot.is_ignored(path) {
                        should_bump = true;
                        break;
                    }
                }
                if should_bump {
                    state_for_watcher.changed();
                }
            }
            Err(_) => {
                state_for_watcher.changed();
            }
        })
        .inspect_err(|_error| {
            #[cfg(test)]
            eprintln!(
                "watcher init failed: {_error}; self_fds={:?}; self_threads={:?}",
                std::fs::read_dir("/proc/self/fd").map(|entries| entries.count()),
                std::fs::read_dir("/proc/self/task").map(|entries| entries.count())
            );
        })
        .ok()?;
    watcher
        .watch(&state.root, RecursiveMode::Recursive)
        .inspect_err(|_error| {
            #[cfg(test)]
            eprintln!("watcher registration failed: {_error}");
        })
        .ok()?;
    Some(watcher)
}

/// The tree's current change generation; also starts watching `root` on first use.
/// Without a working watcher the generation advances on every call, i.e. every query syncs.
pub fn current_generation(root: &Path) -> u64 {
    let lease = lease(root);
    let tracked = &lease.0;
    if tracked._watcher.is_none() {
        tracked.state.changed();
    } else if tracked.state.filter_dirty.swap(false, Ordering::AcqRel) {
        let config = load_config(&tracked.state.root);
        let new_snapshot = Arc::new(IgnoreSnapshot::build(&tracked.state.root, &config.watch));
        if let Ok(mut w) = tracked.state.snapshot.write() {
            *w = new_snapshot;
        }
    }
    tracked.state.generation.load(Ordering::Relaxed)
}

/// Whether a pre-flight sync is due for `root` at `generation` (as returned by
/// [`current_generation`] just before deciding).
pub fn sync_due(root: &Path, generation: u64) -> bool {
    lifecycle::sync_due(root, generation)
}

/// Records that the tree at `generation` was synced successfully.
pub fn mark_synced(root: &Path, generation: u64) {
    lifecycle::mark_synced(root, generation);
}

#[cfg(test)]
pub(crate) fn test_owner(root: &Path) -> impl Fn() -> bool + use<> {
    let owner = Arc::downgrade(&lease(root).0);
    move || owner.upgrade().is_some()
}

#[cfg(test)]
mod tests {
    mod lifecycle;
    use super::*;
    use crate::config::WatchConfig;

    #[test]
    fn generation_advances_on_change_and_sync_is_due_once() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::write(root.join("a.rs"), "a").unwrap();
        let _lease = lease(root);

        let g1 = current_generation(root);
        assert!(sync_due(root, g1), "first contact must sync");
        mark_synced(root, g1);
        assert!(!sync_due(root, g1), "unchanged tree must not sync again");

        std::fs::write(root.join("b.rs"), "b").unwrap();
        // The watcher delivers asynchronously; poll briefly.
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut g2 = current_generation(root);
        while g2 == g1 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
            g2 = current_generation(root);
        }
        assert!(g2 > g1, "a write inside the tree must bump the generation");
        assert!(sync_due(root, g2));
        mark_synced(root, g2);
        let due = sync_due(root, g2);
        // One filesystem write can produce several asynchronous notifications.
        // A later token must invalidate the sync; an unchanged token must not.
        assert!(!due || current_generation(root) != g2);
    }

    #[test]
    fn build_directories_and_vcs_are_ignored() {
        let root = Path::new("/w");
        let config = WatchConfig::default();
        let snapshot = IgnoreSnapshot::build(root, &config);

        assert!(snapshot.is_ignored(Path::new("/w/target/debug/x")));
        assert!(snapshot.is_ignored(Path::new("/w/node_modules/x/y.js")));
        assert!(snapshot.is_ignored(Path::new("/w/.git/HEAD")));
        assert!(snapshot.is_ignored(Path::new("/w/.git/index")));
        assert!(snapshot.is_ignored(Path::new("/w/.idea/workspace.xml")));
        assert!(snapshot.is_ignored(Path::new("/w/.vscode/settings.json")));
        assert!(snapshot.is_ignored(Path::new("/w/.gemini/tmp.log")));
        assert!(snapshot.is_ignored(Path::new("/w/.codex/cache.json")));
        assert!(snapshot.is_ignored(Path::new("/w/.claude/desktop.json")));

        assert!(!snapshot.is_ignored(Path::new("/w/src/main.rs")));
    }
}
