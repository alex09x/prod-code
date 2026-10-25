/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::*;

fn changed(root: &Path, before: u64) -> u64 {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let generation = current_generation(root);
        if generation != before {
            return generation;
        }
        assert!(Instant::now() < deadline, "watcher must observe the write");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn retired_watchers_release_their_real_owners() {
    let mut retired = Vec::new();
    for _ in 0..20 {
        let ws = prod_code_testkit::Workspace::new(&[("a.rs", "a")]);
        let lease = lease(&ws.root());
        assert!(lease.0._watcher.is_some());
        retired.push(Arc::downgrade(&lease.0));
    }
    let live = prod_code_testkit::Workspace::new(&[("a.rs", "a")]);
    current_generation(&live.root());
    assert!(retired.iter().all(|owner| owner.upgrade().is_none()));
}

#[test]
fn evicted_watch_tokens_cannot_acknowledge_a_new_incarnation() {
    let ws = prod_code_testkit::Workspace::new(&[("a.rs", "a")]);
    let old = lease(&ws.root());
    let weak = Arc::downgrade(&old.0);
    let generation = current_generation(&ws.root());
    mark_synced(&ws.root(), generation);
    drop(old);
    let mut roots = Vec::new();
    for _ in 0..24 {
        let other = prod_code_testkit::Workspace::new(&[("a.rs", "a")]);
        current_generation(&other.root());
        roots.push(other);
    }
    assert!(
        weak.upgrade().is_none(),
        "live but idle watchers have a bounded budget"
    );
    let pinned = lease(&ws.root());
    let fresh = current_generation(&ws.root());
    assert_ne!(fresh, generation);
    mark_synced(&ws.root(), generation);
    assert!(
        sync_due(&ws.root(), fresh),
        "late old sync must not acknowledge fresh state"
    );
    mark_synced(&ws.root(), fresh);
    assert!(!sync_due(&ws.root(), fresh));
    assert!(pinned.0._watcher.is_some());
}

#[test]
fn recreated_roots_reject_old_sync_even_with_a_live_old_lease() {
    let ws = prod_code_testkit::Workspace::new(&[("p|雪/a.rs", "a")]);
    let root = ws.path("p|雪");
    let old = lease(&root);
    let generation = current_generation(&root);
    mark_synced(&root, generation);
    std::fs::rename(&root, ws.path("old")).unwrap();
    std::fs::create_dir(&root).unwrap();
    let new = lease(&root);
    let fresh = current_generation(&root);
    assert!(!Arc::ptr_eq(&old.0, &new.0));
    assert_ne!(generation, fresh);
    mark_synced(&root, generation);
    assert!(sync_due(&root, fresh));
    mark_synced(&root, fresh);
    assert!(!sync_due(&root, fresh));
    old.0.state.changed();
    assert_eq!(
        current_generation(&root),
        fresh,
        "old callbacks cannot mutate a replacement"
    );
}

#[test]
fn pinned_watcher_keeps_ignore_and_config_generations() {
    let ws = prod_code_testkit::Workspace::new(&[(".gitignore", "old.txt\n"), ("a.rs", "a")]);
    let root = ws.root();
    let pinned = lease(&root);
    let generation = current_generation(&root);
    assert!(
        pinned
            .0
            .state
            .snapshot
            .read()
            .unwrap()
            .is_ignored(&root.join("old.txt"))
    );
    ws.write(".gitignore", "new.txt\n");
    let next = changed(&root, generation);
    assert!(sync_due(&root, next));
    {
        let snapshot = pinned.0.state.snapshot.read().unwrap();
        assert!(snapshot.is_ignored(&root.join("new.txt")));
        assert!(!snapshot.is_ignored(&root.join("old.txt")));
    }
    ws.write(".prod-code.toml", "[watch]\nignore = [\"config.txt\"]\n");
    changed(&root, next);
    assert!(
        pinned
            .0
            .state
            .snapshot
            .read()
            .unwrap()
            .is_ignored(&root.join("config.txt"))
    );
}

#[test]
fn missing_root_metadata_never_reuses_synced_state() {
    let ws = prod_code_testkit::Workspace::new(&[("a.rs", "a")]);
    let root = ws.root();
    let old = current_generation(&root);
    drop(ws);
    mark_synced(&root, old);
    assert!(sync_due(&root, old));
    let fresh = current_generation(&root);
    mark_synced(&root, fresh);
    assert!(sync_due(&root, fresh));
    assert_ne!(fresh, current_generation(&root));
}

#[test]
fn stale_observations_preserve_inserted_and_replaced_incarnations_until_over_budget() {
    let registry = lifecycle::Registry::default();
    let replaced_ws = prod_code_testkit::Workspace::new(&[("r/a.rs", "a")]);
    let replaced_root = replaced_ws.path("r");
    let deleted_ws = prod_code_testkit::Workspace::new(&[("a.rs", "a")]);
    let deleted_root = deleted_ws.root();
    let reborn_ws = prod_code_testkit::Workspace::new(&[("b/a.rs", "a")]);
    let reborn_root = reborn_ws.path("b");
    let inserted_ws = prod_code_testkit::Workspace::new(&[("a.rs", "a")]);
    let inserted_root = inserted_ws.root();
    let trigger_ws = prod_code_testkit::Workspace::new(&[("a.rs", "a")]);
    let trigger_root = trigger_ws.root();

    let replaced_old = lifecycle::lease_in(&registry, &replaced_root);
    let replaced_old_gen = replaced_old.0.state.generation.load(Ordering::Relaxed);
    let replaced_old_weak = Arc::downgrade(&replaced_old.0);
    lifecycle::mark_synced_in(&registry, &replaced_root, replaced_old_gen);
    drop(replaced_old);

    let deleted = lifecycle::lease_in(&registry, &deleted_root);
    let deleted_gen = deleted.0.state.generation.load(Ordering::Relaxed);
    let deleted_weak = Arc::downgrade(&deleted.0);
    lifecycle::mark_synced_in(&registry, &deleted_root, deleted_gen);
    drop(deleted);

    let reborn_old = lifecycle::lease_in(&registry, &reborn_root);
    let reborn_old_weak = Arc::downgrade(&reborn_old.0);
    drop(reborn_old);

    let prior_observations = lifecycle::observe_registry(&registry);
    let incarnations = lifecycle::snapshot_incarnations(&registry);
    drop(deleted_ws);
    std::fs::remove_dir_all(&reborn_root).unwrap();
    let stale_observations = lifecycle::observe_identities(incarnations);

    let inserted = lifecycle::lease_observed(&registry, &inserted_root, &prior_observations);
    let inserted_gen = inserted.0.state.generation.load(Ordering::Relaxed);
    let inserted_weak = Arc::downgrade(&inserted.0);
    drop(inserted);

    std::fs::rename(&replaced_root, replaced_ws.path("r_old")).unwrap();
    std::fs::create_dir(&replaced_root).unwrap();
    let replaced_new = lifecycle::lease_observed(&registry, &replaced_root, &prior_observations);
    let replaced_new_gen = replaced_new.0.state.generation.load(Ordering::Relaxed);
    let replaced_new_weak = Arc::downgrade(&replaced_new.0);
    drop(replaced_new);
    assert!(replaced_old_weak.upgrade().is_none());
    assert_ne!(replaced_new_gen, replaced_old_gen);

    std::fs::create_dir(&reborn_root).unwrap();
    let reborn_new = lifecycle::lease_observed(&registry, &reborn_root, &prior_observations);
    let reborn_new_gen = reborn_new.0.state.generation.load(Ordering::Relaxed);
    let reborn_new_weak = Arc::downgrade(&reborn_new.0);
    drop(reborn_new);
    assert!(reborn_old_weak.upgrade().is_none());
    assert!(deleted_weak.upgrade().is_some());

    let trigger = lifecycle::lease_observed(&registry, &trigger_root, &stale_observations);
    let trigger_weak = Arc::downgrade(&trigger.0);
    assert!(
        deleted_weak.upgrade().is_none(),
        "observed incarnation with absent metadata must be retired"
    );
    assert!(lifecycle::sync_due_in(
        &registry,
        &deleted_root,
        deleted_gen
    ));
    assert!(
        inserted_weak.upgrade().is_some(),
        "entry inserted after observation must be treated as unknown"
    );
    lifecycle::mark_synced_in(&registry, &inserted_root, inserted_gen);
    assert!(!lifecycle::sync_due_in(
        &registry,
        &inserted_root,
        inserted_gen
    ));
    assert!(
        replaced_new_weak.upgrade().is_some(),
        "replaced incarnation must not be retired by prior incarnation identity"
    );
    lifecycle::mark_synced_in(&registry, &replaced_root, replaced_old_gen);
    assert!(lifecycle::sync_due_in(
        &registry,
        &replaced_root,
        replaced_new_gen
    ));
    lifecycle::mark_synced_in(&registry, &replaced_root, replaced_new_gen);
    assert!(!lifecycle::sync_due_in(
        &registry,
        &replaced_root,
        replaced_new_gen
    ));
    assert!(
        reborn_new_weak.upgrade().is_some(),
        "replaced incarnation must not inherit missing metadata from prior incarnation"
    );
    lifecycle::mark_synced_in(&registry, &reborn_root, reborn_new_gen);
    assert!(!lifecycle::sync_due_in(
        &registry,
        &reborn_root,
        reborn_new_gen
    ));
    drop(trigger);

    let current_observations = lifecycle::observe_registry(&registry);
    lifecycle::retire_idle(&registry, 1, &current_observations);
    assert!(
        inserted_weak.upgrade().is_none()
            && replaced_new_weak.upgrade().is_none()
            && reborn_new_weak.upgrade().is_none()
            && trigger_weak.upgrade().is_some(),
        "once over the idle budget, normal capacity eviction retires older idle roots"
    );
}
