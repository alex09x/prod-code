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
    synced_generation: u64,
    last_sync: Option<Instant>,
    _watcher: Option<RecommendedWatcher>,
}

fn registry() -> &'static Mutex<HashMap<PathBuf, Tracked>> {
    static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, Tracked>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn start_watcher(state: Arc<WatchState>) -> Option<RecommendedWatcher> {
    let root = state.root.clone();
    let state_for_watcher = Arc::clone(&state);
    let mut watcher =
        notify::recommended_watcher(move |event: notify::Result<notify::Event>| match event {
            Ok(event) => {
                if event.paths.is_empty() {
                    state_for_watcher.generation.fetch_add(1, Ordering::Relaxed);
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
                    state_for_watcher.generation.fetch_add(1, Ordering::Relaxed);
                }
            }
            Err(_) => {
                state_for_watcher.generation.fetch_add(1, Ordering::Relaxed);
            }
        })
        .ok()?;
    watcher.watch(&state.root, RecursiveMode::Recursive).ok()?;
    Some(watcher)
}

/// The tree's current change generation; also starts watching `root` on first use.
/// Without a working watcher the generation advances on every call, i.e. every query syncs.
pub fn current_generation(root: &Path) -> u64 {
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut registry = registry().lock().unwrap_or_else(|e| e.into_inner());
    let tracked = registry.entry(root.clone()).or_insert_with(|| {
        let generation = Arc::new(AtomicU64::new(1));
        let filter_dirty = Arc::new(AtomicBool::new(false));
        let config = load_config(&root);
        let snapshot = RwLock::new(Arc::new(IgnoreSnapshot::build(&root, &config.watch)));
        let state = Arc::new(WatchState {
            root: root.clone(),
            generation,
            filter_dirty,
            snapshot,
        });
        let watcher = start_watcher(Arc::clone(&state));
        Tracked {
            state,
            synced_generation: 0,
            last_sync: None,
            _watcher: watcher,
        }
    });

    if tracked._watcher.is_none() {
        tracked.state.generation.fetch_add(1, Ordering::Relaxed);
    } else if tracked.state.filter_dirty.swap(false, Ordering::AcqRel) {
        let config = load_config(&root);
        let new_snapshot = Arc::new(IgnoreSnapshot::build(&root, &config.watch));
        if let Ok(mut w) = tracked.state.snapshot.write() {
            *w = new_snapshot;
        }
    }
    tracked.state.generation.load(Ordering::Relaxed)
}

/// Whether a pre-flight sync is due for `root` at `generation` (as returned by
/// [`current_generation`] just before deciding).
pub fn sync_due(root: &Path, generation: u64) -> bool {
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let registry = registry().lock().unwrap_or_else(|e| e.into_inner());
    match registry.get(&root) {
        Some(t) => {
            t.synced_generation != generation
                || t.last_sync.is_none_or(|at| at.elapsed() >= MAX_SYNC_AGE)
        }
        None => true,
    }
}

/// Records that the tree at `generation` was synced successfully.
pub fn mark_synced(root: &Path, generation: u64) {
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut registry = registry().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(t) = registry.get_mut(&root) {
        t.synced_generation = generation;
        t.last_sync = Some(Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WatchConfig;

    #[test]
    fn generation_advances_on_change_and_sync_is_due_once() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::write(root.join("a.rs"), "a").unwrap();

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
        assert!(!sync_due(root, g2));
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
