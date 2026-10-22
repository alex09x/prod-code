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

fn engine() -> (
    tempfile::TempDir,
    Arc<tokio::sync::Mutex<prod_code_engine_rust::RustEngine>>,
    PathBuf,
) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Cargo.toml"),
        "[package]\nname = \"validation_fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[lib]\npath = \"lib.rs\"\n").unwrap();
    std::fs::write(
        dir.path().join("prod-code.toml"),
        "[rust]\nbuild_scripts = false\nproc_macro_srv = \"disabled\"\n",
    )
    .unwrap();
    let path = dir.path().join("lib.rs");
    std::fs::write(&path, "pub fn original() {}\n").unwrap();
    let engine = prod_code_engine_rust::RustEngine::load(dir.path()).unwrap();
    (dir, Arc::new(tokio::sync::Mutex::new(engine)), path)
}

#[test]
fn validation_query_budget_is_clamped() {
    assert_eq!(
        semantic_query_budget(&serde_json::json!({})),
        SEMANTIC_QUERY_TIMEOUT
    );
    assert_eq!(
        semantic_query_budget(&serde_json::json!({"prodCodeTimeoutMs": 7})),
        Duration::from_millis(7)
    );
    assert_eq!(
        semantic_query_budget(&serde_json::json!({"prodCodeTimeoutMs": 0})),
        Duration::from_millis(1)
    );
    assert_eq!(
        semantic_query_budget(&serde_json::json!({"prodCodeTimeoutMs": u64::MAX})),
        SEMANTIC_QUERY_TIMEOUT
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn validation_deadline_releases_snapshot_before_blocking_closure_returns() {
    let (_dir, engine, path) = engine();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let release_rx = Arc::new(std::sync::Mutex::new(release_rx));
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let started_tx = Arc::new(std::sync::Mutex::new(Some(started_tx)));
    let (cancelled_tx, cancelled_rx) = std::sync::mpsc::channel();
    let query_path = path.clone();
    let query = execute_bounded_query_with_timeout(
        &engine,
        1,
        &path,
        true,
        Duration::from_millis(100),
        move |snapshot| {
            started_tx.lock().unwrap().take().unwrap().send(()).unwrap();
            release_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
            cancelled_tx
                .send(snapshot.document_symbols(&query_path).is_err())
                .unwrap();
            release_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
            Ok(())
        },
    );
    let (result, started) = tokio::join!(query, started_rx);
    started.unwrap();
    assert!(result.unwrap_err().to_string().contains("timed out"));
    release_tx.send(()).unwrap();
    let cancelled_before_write = cancelled_rx.recv_timeout(Duration::from_secs(2));
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let writer_engine = Arc::clone(&engine);
    let writer = std::thread::spawn(move || {
        let result = writer_engine.blocking_lock().set_session_overlay(
            1,
            &path,
            Some("pub fn updated() {}\n".into()),
        );
        done_tx.send(result).unwrap();
    });
    let completed = done_rx.recv_timeout(Duration::from_millis(500));
    // Always release and join, including on the unfixed implementation.
    release_tx.send(()).unwrap();
    writer.join().unwrap();
    assert!(
        cancelled_before_write.unwrap(),
        "deadline did not revoke idle snapshot"
    );
    completed
        .expect("deadline left a Salsa snapshot in the blocking task")
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn validation_retries_more_than_three_cancellations_on_fresh_snapshots() {
    let (_dir, engine, path) = engine();
    let attempts = Arc::new(AtomicUsize::new(0));
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let started = Arc::new(std::sync::Mutex::new(Some(started_tx)));
    let (changed_tx, changed_rx) = std::sync::mpsc::channel();
    let changed = Arc::new(std::sync::Mutex::new(changed_rx));
    let query = execute_bounded_query(&engine, 1, &path, true, {
        let path = path.clone();
        let attempts = Arc::clone(&attempts);
        move |snapshot| {
            let attempt = attempts.fetch_add(1, Ordering::SeqCst);
            if attempt == 0 {
                started.lock().unwrap().take().unwrap().send(()).unwrap();
                changed
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(10))
                    .unwrap();
            }
            if attempt < 4 {
                anyhow::bail!("cancelled because of pending write");
            }
            Ok(snapshot
                .document_symbols(&path)?
                .into_iter()
                .map(|s| s.name)
                .collect::<Vec<_>>())
        }
    });
    let mutation = async {
        started_rx.await.unwrap();
        let mut engine = engine.lock().await;
        // Salsa waits for the old snapshot to drop before completing this write. Release
        // the simulated cancelled query first; waiting for the write here would deadlock.
        changed_tx.send(()).unwrap();
        engine
            .set_session_overlay(1, &path, Some("pub fn updated() {}\n".into()))
            .unwrap();
    };
    let (result, ()) = tokio::join!(query, mutation);
    assert_eq!(result.unwrap(), ["updated"]);
    assert_eq!(attempts.load(Ordering::SeqCst), 5);
}

#[tokio::test]
async fn validation_query_budget_includes_waiting_for_engine_lock() {
    let (_dir, engine, path) = engine();
    let _held = engine.lock().await;
    let result = execute_bounded_query_with_timeout(
        &engine,
        1,
        &path,
        true,
        Duration::from_millis(20),
        |_| Ok(()),
    )
    .await;
    let message = result.unwrap_err().to_string();
    assert!(
        message.contains("20ms waiting for engine lock"),
        "{message}"
    );
}

#[tokio::test]
async fn validation_repeated_cancellation_finishes_as_a_bounded_timeout() {
    let (_dir, engine, path) = engine();
    let result = execute_bounded_query_with_timeout(
        &engine,
        1,
        &path,
        true,
        Duration::from_millis(20),
        |_| Err::<(), _>(anyhow::anyhow!("cancelled because of pending write")),
    )
    .await;
    let message = result.unwrap_err().to_string();
    assert!(
        message.starts_with("query timed out after 20ms"),
        "{message}"
    );
    assert!(!message.contains("pending write"), "{message}");
}
