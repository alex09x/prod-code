/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::sync::Weak;

use super::*;

pub(super) const MAX_IDLE: usize = 16;
const MAX_IDLE_AGE: Duration = Duration::from_secs(300);

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct RootIdentity {
    created: Option<std::time::SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

pub(crate) fn root_identity(root: &Path) -> Option<RootIdentity> {
    let metadata = std::fs::metadata(root).ok()?;
    if !metadata.is_dir() {
        return None;
    }
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    // Without a stable identity, force revalidation rather than trust a replaced root.
    #[cfg(not(unix))]
    let created = Some(metadata.created().ok()?);
    #[cfg(unix)]
    let created = metadata.created().ok();
    Some(RootIdentity {
        created,
        #[cfg(unix)]
        device: metadata.dev(),
        #[cfg(unix)]
        inode: metadata.ino(),
    })
}

pub(super) struct Entry {
    tracked: Arc<Tracked>,
    identity: Option<RootIdentity>,
    last_used: Instant,
}

pub(super) type Registry = Mutex<HashMap<PathBuf, Entry>>;

pub(super) struct Observation {
    tracked: Weak<Tracked>,
    identity: Option<RootIdentity>,
}

fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) fn snapshot_incarnations(registry: &Registry) -> Vec<(PathBuf, Weak<Tracked>)> {
    registry
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .map(|(key, entry)| (key.clone(), Arc::downgrade(&entry.tracked)))
        .collect()
}

pub(super) fn observe_identities(
    incarnations: Vec<(PathBuf, Weak<Tracked>)>,
) -> HashMap<PathBuf, Observation> {
    incarnations
        .into_iter()
        .map(|(key, tracked)| {
            let identity = root_identity(&key);
            (key, Observation { tracked, identity })
        })
        .collect()
}

pub(super) fn observe_registry(registry: &Registry) -> HashMap<PathBuf, Observation> {
    observe_identities(snapshot_incarnations(registry))
}

fn entry_invalid(key: &Path, entry: &Entry, observations: &HashMap<PathBuf, Observation>) -> bool {
    entry.identity.is_none()
        || observations.get(key).is_some_and(|observed| {
            observed.tracked.ptr_eq(&Arc::downgrade(&entry.tracked))
                && (observed.identity.is_none() || observed.identity != entry.identity)
        })
}

// A session (including queued queries) pins its watcher independently of the idle budget.
pub(crate) struct Lease(pub(super) Arc<Tracked>);

impl Drop for Lease {
    fn drop(&mut self) {
        let identity = root_identity(&self.0.state.root);
        let mut registry = registry().lock().unwrap_or_else(|e| e.into_inner());
        let mut idle: Vec<_> = registry
            .iter()
            .filter(|(_, entry)| {
                let owners = Arc::strong_count(&entry.tracked);
                owners == 1 || (owners == 2 && Arc::ptr_eq(&self.0, &entry.tracked))
            })
            .map(|(key, entry)| (key.clone(), entry.last_used))
            .collect();
        idle.sort_by_key(|(_, at)| *at);
        let excess = idle.len().saturating_sub(MAX_IDLE);
        let retired: Vec<_> = idle
            .into_iter()
            .enumerate()
            .filter_map(|(index, (key, _))| {
                let invalid = registry.get(&key).is_some_and(|entry| {
                    entry.identity.is_none()
                        || (Arc::ptr_eq(&self.0, &entry.tracked)
                            && (identity.is_none() || identity != entry.identity))
                });
                if index < excess || invalid {
                    registry.remove(&key)
                } else {
                    None
                }
            })
            .collect();
        drop(registry);
        drop(retired);
    }
}

fn retire_idle_locked(
    registry: &mut HashMap<PathBuf, Entry>,
    budget: usize,
    observations: &HashMap<PathBuf, Observation>,
    retired: &mut Vec<Entry>,
) {
    let mut idle: Vec<_> = registry
        .iter()
        .filter(|(_, entry)| Arc::strong_count(&entry.tracked) == 1)
        .map(|(key, entry)| (key.clone(), entry.last_used))
        .collect();
    idle.sort_by_key(|(_, at)| *at);
    let excess = idle.len().saturating_sub(budget);
    for (index, (key, at)) in idle.into_iter().enumerate() {
        let invalid = registry
            .get(&key)
            .is_some_and(|entry| entry_invalid(&key, entry, observations));
        if index < excess || invalid || at.elapsed() >= MAX_IDLE_AGE {
            retired.extend(registry.remove(&key));
        }
    }
}

#[cfg(test)]
pub(super) fn retire_idle(
    registry: &Registry,
    budget: usize,
    observations: &HashMap<PathBuf, Observation>,
) {
    let mut registry = registry.lock().unwrap_or_else(|e| e.into_inner());
    let mut retired = Vec::new();
    retire_idle_locked(&mut registry, budget, observations, &mut retired);
    drop(registry);
    drop(retired);
}

pub(super) fn lease_observed(
    registry: &Registry,
    root: &Path,
    observations: &HashMap<PathBuf, Observation>,
) -> Lease {
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let identity = root_identity(&root);
    let existing = {
        let mut registry = registry.lock().unwrap_or_else(|e| e.into_inner());
        registry
            .get_mut(&root)
            .filter(|entry| identity.is_some() && entry.identity == identity)
            .map(|entry| {
                entry.last_used = Instant::now();
                Arc::clone(&entry.tracked)
            })
    };
    // Construct and start outside the registry lock. Concurrent insertion losers are also
    // destroyed outside it, since notify's destructor can join a callback thread.
    let candidate = existing.unwrap_or_else(|| Arc::new(Tracked::new(&root)));
    let mut registry = registry.lock().unwrap_or_else(|e| e.into_inner());
    let mut retired = Vec::new();
    if registry
        .get(&root)
        .is_some_and(|entry| identity.is_none() || entry.identity != identity)
    {
        retired.extend(registry.remove(&root));
    }
    let entry = registry.entry(root.clone()).or_insert_with(|| Entry {
        tracked: Arc::clone(&candidate),
        identity,
        last_used: Instant::now(),
    });
    entry.last_used = Instant::now();
    let tracked = Arc::clone(&entry.tracked);
    retire_idle_locked(
        &mut registry,
        MAX_IDLE.saturating_sub(1),
        observations,
        &mut retired,
    );
    drop(registry);
    drop(retired);
    drop(candidate);
    Lease(tracked)
}

pub(super) fn lease_in(registry: &Registry, root: &Path) -> Lease {
    let observations = observe_registry(registry);
    lease_observed(registry, root, &observations)
}

pub(crate) fn lease(root: &Path) -> Lease {
    lease_in(registry(), root)
}

pub(super) fn tracked_in(registry: &Registry, root: &Path) -> Option<Arc<Tracked>> {
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let identity = root_identity(&root);
    registry
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&root)
        .filter(|entry| identity.is_some() && entry.identity == identity)
        .map(|entry| Arc::clone(&entry.tracked))
}

pub(super) fn sync_due_in(registry: &Registry, root: &Path, generation: u64) -> bool {
    tracked_in(registry, root).is_none_or(|tracked| {
        let sync = tracked.synced.lock().unwrap_or_else(|e| e.into_inner());
        tracked.state.generation.load(Ordering::Relaxed) != generation
            || sync.0 != generation
            || sync.1.is_none_or(|at| at.elapsed() >= MAX_SYNC_AGE)
    })
}

pub(super) fn sync_due(root: &Path, generation: u64) -> bool {
    sync_due_in(registry(), root, generation)
}

pub(super) fn mark_synced_in(registry: &Registry, root: &Path, generation: u64) {
    if let Some(tracked) = tracked_in(registry, root) {
        let mut sync = tracked.synced.lock().unwrap_or_else(|e| e.into_inner());
        // Tokens are process-unique, including across eviction and same-path recreation.
        if tracked.state.generation.load(Ordering::Relaxed) == generation {
            *sync = (generation, Some(Instant::now()));
        }
    }
}

pub(super) fn mark_synced(root: &Path, generation: u64) {
    mark_synced_in(registry(), root, generation);
}
