/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::fixtures::{host, loaded, manager_on};

/// Six worktrees handshake at once on a host with 80 of 100 GiB in use: 5 GiB are left
/// under the 85% limit, so two new engines of 2 GiB load and four are refused for capacity,
/// while a session of the engine already loaded attaches as before (#433).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn simultaneous_new_engines_load_only_while_memory_lasts() {
    let manager = manager_on(vec![host(80, 100)]);
    manager
        .insert_ready_for_test(loaded("/srv/ws/warm", 0))
        .await;
    let start = Arc::new(tokio::sync::Barrier::new(6));
    let loads: Vec<_> = (0..6)
        .map(|i| {
            let manager = Arc::clone(&manager);
            let start = Arc::clone(&start);
            tokio::spawn(async move {
                start.wait().await;
                manager
                    .get_or_load(&PathBuf::from(format!("/srv/ws/new-{i}")), "text")
                    .await
            })
        })
        .collect();
    let mut refusals = Vec::new();
    let mut admitted = 0;
    for load in loads {
        match load.await.unwrap() {
            Ok(_) => admitted += 1,
            Err(err) => refusals.push(err.to_string()),
        }
    }
    assert_eq!(admitted, 2, "{refusals:?}");
    assert_eq!(refusals.len(), 4);
    for refusal in &refusals {
        assert!(refusal.starts_with("capacity: "), "{refusal}");
        assert!(refusal.contains("another node"), "{refusal}");
    }
    assert_eq!(manager.admission().reserved_bytes(), 4 << 30);
    assert_eq!(
        manager.loaded_count().await,
        3,
        "no refused load is left behind"
    );

    let warm = manager
        .get_or_load(Path::new("/srv/ws/warm"), "text")
        .await
        .expect("a loaded engine is used whatever the memory");
    assert_eq!(warm.active_sessions.load(Ordering::Relaxed), 1);
}

/// A new engine that finds no room unloads the least recently used engine that has been
/// idle for a while, as many as it needs, and loads once the host shows the memory back;
/// engines with a session or used a moment ago stay (#433).
#[tokio::test]
async fn a_new_engine_makes_room_by_unloading_only_idle_engines() {
    // 84% in use: a new 2 GiB engine passes the limit by 1 GiB, until one is unloaded.
    let manager = manager_on(vec![host(84, 100), host(82, 100)]);
    let oldest = loaded("/srv/ws/oldest", 7200);
    let older = loaded("/srv/ws/older", 3600);
    let in_use = loaded("/srv/ws/in-use", 9000);
    in_use.active_sessions.store(1, Ordering::Relaxed);
    let recent = loaded("/srv/ws/recent", 30);
    let gone = Arc::downgrade(&oldest);
    for ws in [oldest, older, in_use, recent] {
        manager.insert_ready_for_test(ws).await;
    }

    manager
        .get_or_load(Path::new("/srv/ws/new"), "text")
        .await
        .expect("room was made");
    assert!(!manager.is_loaded(Path::new("/srv/ws/oldest")).await);
    assert!(gone.upgrade().is_none(), "its engine is freed");
    for kept in ["older", "in-use", "recent", "new"] {
        assert!(
            manager.is_loaded(&Path::new("/srv/ws").join(kept)).await,
            "{kept} was unloaded"
        );
    }
}

/// With nothing idle long enough to unload, a new engine is refused and the map is left as
/// it was; so is it when the engines unloaded did not give the memory back.
#[tokio::test]
async fn a_new_engine_is_refused_when_unloading_cannot_make_room() {
    let manager = manager_on(vec![host(90, 100)]);
    let in_use = loaded("/srv/ws/in-use", 9000);
    in_use.active_sessions.store(1, Ordering::Relaxed);
    manager.insert_ready_for_test(in_use).await;
    manager
        .insert_ready_for_test(loaded("/srv/ws/recent", 30))
        .await;
    let Err(err) = manager.get_or_load(Path::new("/srv/ws/new"), "text").await else {
        panic!("admitted without room");
    };
    let refused = err
        .downcast_ref::<crate::admission::CapacityRefused>()
        .expect("refused for capacity");
    assert_eq!(refused.reclaimed, 0);
    assert!(!manager.is_loaded(Path::new("/srv/ws/new")).await);
    assert_eq!(manager.loaded_count().await, 2);
    assert_eq!(manager.admission().reserved_bytes(), 0);

    let manager = manager_on(vec![host(90, 100)]);
    manager
        .insert_ready_for_test(loaded("/srv/ws/idle", 7200))
        .await;
    let Err(err) = manager.get_or_load(Path::new("/srv/ws/new"), "text").await else {
        panic!("admitted though unloading gave no memory back");
    };
    let text = err.to_string();
    assert!(text.contains("1 idle engine(s) were unloaded"), "{text}");
    assert_eq!(manager.loaded_count().await, 0);
}
