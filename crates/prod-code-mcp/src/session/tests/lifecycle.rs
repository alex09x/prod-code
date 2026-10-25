/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::pool::{pool_key, pooled_query, session_slot};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

fn descriptors() -> Option<usize> {
    std::fs::read_dir("/proc/self/fd").ok().map(Iterator::count)
}

async fn ask(gateway: &prod_code_testkit::ScriptedGateway, root: &std::path::Path) {
    pooled_query(
        gateway.addr(),
        root,
        &root.join("src/lib.rs"),
        "textDocument/documentSymbol",
        serde_json::json!({}),
    )
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn warm_roots_and_exact_path_identity_survive_retirement() {
    let handshakes = Arc::new(AtomicUsize::new(0));
    let count = handshakes.clone();
    let gateway = prod_code_testkit::ScriptedGateway::start(move |method, _| {
        if method == "prod-code/handshake" {
            count.fetch_add(1, Ordering::SeqCst);
        }
        serde_json::json!([])
    })
    .await;
    let ws = prod_code_testkit::Workspace::new(&[("p|雪/src/lib.rs", "pub fn a() {}\n")]);
    let root = ws.path("p|雪");
    ask(&gateway, &root).await;
    let (_, key) = pool_key(gateway.addr(), &root, &root.join("src/lib.rs"));
    // Own the warm slot while unrelated parallel tests apply process-wide cache pressure.
    let pinned = session_slot(&key).await;
    let old = Arc::downgrade(&pinned);
    #[cfg(unix)]
    {
        let alias = ws.path("alias");
        std::os::unix::fs::symlink(&root, &alias).unwrap();
        ask(&gateway, &alias).await;
        let (_, alias_key) = pool_key(gateway.addr(), &alias, &alias.join("src/lib.rs"));
        assert_eq!(key, alias_key);
    }
    for _ in 0..4 {
        let retired = prod_code_testkit::Workspace::new(&[("src/lib.rs", "pub fn b() {}\n")]);
        ask(&gateway, &retired.root()).await;
    }
    ask(&gateway, &root).await;
    assert!(
        old.upgrade().is_some(),
        "a warm live root retains its socket"
    );
    assert_eq!(handshakes.load(Ordering::SeqCst), 5);
    drop(pinned);

    // Keep the old inode alive elsewhere, ensuring same-path recreation is unambiguous.
    let moved = ws.path("old");
    std::fs::rename(&root, &moved).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn replacement() {}\n").unwrap();
    ask(&gateway, &root).await;
    assert!(old.upgrade().is_none());
    assert_eq!(
        handshakes.load(Ordering::SeqCst),
        6,
        "replacement needs a new session"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn active_and_queued_queries_survive_idle_cache_pressure() {
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(AtomicBool::new(false));
    let blocked = Arc::new(AtomicBool::new(false));
    let (notify, done, block) = (entered.clone(), release.clone(), blocked.clone());
    let gateway = prod_code_testkit::ScriptedGateway::start(move |method, _| {
        if method == "textDocument/documentSymbol" && block.swap(false, Ordering::SeqCst) {
            notify.notify_one();
            tokio::task::block_in_place(|| {
                let deadline = std::time::Instant::now() + Duration::from_secs(15);
                while !done.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(5));
                }
            });
        }
        serde_json::json!([])
    })
    .await;
    let ws = prod_code_testkit::Workspace::new(&[("src/lib.rs", "pub fn a() {}\n")]);
    ask(&gateway, &ws.root()).await;
    let (_, key) = pool_key(gateway.addr(), &ws.root(), &ws.path("src/lib.rs"));
    let owner = Arc::downgrade(&session_slot(&key).await);
    blocked.store(true, Ordering::SeqCst);
    let run = || {
        let (root, addr) = (ws.root(), gateway.addr());
        tokio::spawn(async move {
            pooled_query(
                addr,
                &root,
                &root.join("src/lib.rs"),
                "textDocument/documentSymbol",
                serde_json::json!({}),
            )
            .await
        })
    };
    let active = run();
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    let queued = run();
    tokio::time::timeout(Duration::from_secs(5), async {
        while owner.strong_count() < 3 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let mut idle = Vec::new();
    let mut roots = Vec::new();
    for _ in 0..24 {
        let other = prod_code_testkit::Workspace::new(&[("src/lib.rs", "pub fn b() {}\n")]);
        ask(&gateway, &other.root()).await;
        let (_, key) = pool_key(gateway.addr(), &other.root(), &other.path("src/lib.rs"));
        idle.push(Arc::downgrade(&session_slot(&key).await));
        roots.push(other);
    }
    release.store(true, Ordering::SeqCst);
    active.await.unwrap().unwrap();
    queued.await.unwrap().unwrap();
    assert!(
        owner.upgrade().is_some(),
        "pressure must not evict the query owner"
    );
    assert!(
        idle.iter().filter(|slot| slot.upgrade().is_some()).count()
            <= super::super::lifecycle::MAX_IDLE
    );
    assert!(
        idle[0].upgrade().is_none(),
        "old idle live roots are bounded too"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reused_socket_revalidates_changed_text() {
    let changes = Arc::new(AtomicUsize::new(0));
    let count = changes.clone();
    let gateway = prod_code_testkit::ScriptedGateway::start(move |method, _| {
        if method == "textDocument/didChange" {
            count.fetch_add(1, Ordering::SeqCst);
        }
        serde_json::json!([])
    })
    .await;
    let ws = prod_code_testkit::Workspace::new(&[("src/lib.rs", "pub fn a() {}\n")]);
    ask(&gateway, &ws.root()).await;
    let before = crate::watch::current_generation(&ws.root());
    ws.write("src/lib.rs", "pub fn changed() {}\n");
    tokio::time::timeout(Duration::from_secs(5), async {
        while crate::watch::current_generation(&ws.root()) == before {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    ask(&gateway, &ws.root()).await;
    assert_eq!(
        changes.load(Ordering::SeqCst),
        1,
        "refresh must deliver new text to the real socket"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelled_queries_retire_deleted_roots_without_a_later_query() {
    let entered = Arc::new(tokio::sync::Notify::new());
    let blocked = Arc::new(AtomicBool::new(false));
    let (notify, block) = (entered.clone(), blocked.clone());
    let gateway = prod_code_testkit::ScriptedGateway::start(move |method, _| {
        if method == "textDocument/documentSymbol" && block.load(Ordering::SeqCst) {
            notify.notify_one();
            return serde_json::json!({"prod-code/no-response": true});
        }
        serde_json::json!([])
    })
    .await;
    let ws = prod_code_testkit::Workspace::new(&[("src/lib.rs", "pub fn a() {}\n")]);
    let root = ws.root();
    ask(&gateway, &root).await;
    let (_, key) = pool_key(gateway.addr(), &root, &root.join("src/lib.rs"));
    let owner = Arc::downgrade(&session_slot(&key).await);
    let watcher = crate::watch::test_owner(&root);
    blocked.store(true, Ordering::SeqCst);
    let addr = gateway.addr();
    let running = tokio::spawn(async move {
        pooled_query(
            addr,
            &root,
            &root.join("src/lib.rs"),
            "textDocument/documentSymbol",
            serde_json::json!({}),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    drop(ws);
    running.abort();
    assert!(running.await.unwrap_err().is_cancelled());
    assert!(owner.upgrade().is_none());
    assert!(
        !watcher(),
        "cancellation must release the last watcher lease too"
    );
}

// Semantic replies are transport fixtures; sockets, sync, watchers and client caches are real.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retired_roots_release_real_client_owners() {
    let gateway = prod_code_testkit::ScriptedGateway::start(|_, _| serde_json::json!([])).await;
    let before = descriptors();
    let mut retired = Vec::new();
    let mut watchers = Vec::new();
    #[cfg(target_os = "linux")]
    let mut sockets = Vec::new();
    for _ in 0..20 {
        let ws = prod_code_testkit::Workspace::new(&[("src/lib.rs", "pub fn a() {}\n")]);
        let root = ws.root();
        let file = ws.path("src/lib.rs");
        pooled_query(
            gateway.addr(),
            &root,
            &file,
            "textDocument/documentSymbol",
            serde_json::json!({}),
        )
        .await
        .unwrap();
        let (_, key) = pool_key(gateway.addr(), &root, &file);
        let slot = session_slot(&key).await;
        retired.push(Arc::downgrade(&slot));
        watchers.push(crate::watch::test_owner(&root));
        #[cfg(target_os = "linux")]
        {
            use prod_code_protocol::AnyStream;
            use std::os::fd::AsRawFd;
            let session = slot.lock().await;
            let stream = session.as_ref().unwrap().framed.get_ref();
            let fd = match stream {
                AnyStream::Tcp(s) => s.as_raw_fd(),
                AnyStream::TlsClient(s) => s.get_ref().0.as_raw_fd(),
                AnyStream::TlsServer(s) => s.get_ref().0.as_raw_fd(),
                AnyStream::Unix(s) => s.as_raw_fd(),
            };
            sockets.push(std::fs::read_link(format!("/proc/self/fd/{fd}")).unwrap());
        }
        // No explicit cache cleanup or gateway disconnect: only ordinary root retirement.
    }
    let live = prod_code_testkit::Workspace::new(&[("src/lib.rs", "pub fn a() {}\n")]);
    pooled_query(
        gateway.addr(),
        &live.root(),
        &live.path("src/lib.rs"),
        "textDocument/documentSymbol",
        serde_json::json!({}),
    )
    .await
    .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let retained = retired
        .iter()
        .filter(|slot| slot.upgrade().is_some())
        .count();
    eprintln!(
        "retire cycles=20 retained={retained} descriptors before={before:?} after={:?}; no cleanup/disconnect/limit changes",
        descriptors()
    );
    assert_eq!(
        retained, 0,
        "removed roots must release the pool's strong owners"
    );
    assert!(
        watchers.iter().all(|alive| !alive()),
        "client retirement must release real watchers"
    );
    #[cfg(target_os = "linux")]
    {
        let open: Vec<_> = std::fs::read_dir("/proc/self/fd")
            .unwrap()
            .filter_map(|fd| std::fs::read_link(fd.ok()?.path()).ok())
            .collect();
        let retained = sockets
            .iter()
            .filter(|socket| open.contains(socket))
            .count();
        eprintln!("retired client socket descriptors still open={retained}/20");
        assert_eq!(
            retained, 0,
            "retired sockets must close even while unrelated queries are active"
        );
    }
}
