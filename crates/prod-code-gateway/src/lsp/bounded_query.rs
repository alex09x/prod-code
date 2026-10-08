/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::*;

/// Maximum duration a semantic read query (references, definition, hover, call hierarchy,
/// symbols) may run before being terminated with a server-side timeout error.
pub(crate) const SEMANTIC_QUERY_TIMEOUT: Duration = Duration::from_secs(60);

/// Maximum transparent retries when a snapshot query is cancelled by a concurrent mutation
/// (e.g. an edit or sync mutating the Salsa database).
pub(crate) const MAX_CANCELLATION_RETRIES: usize = 3;

/// Maximum concurrent semantic read queries executing in the blocking pool (#3111).
pub(crate) const MAX_CONCURRENT_SEMANTIC_QUERIES: usize = 16;

/// Global semaphore bounding concurrent semantic queries to strictly limit the number
/// of outstanding blocking query tasks, preventing exhaustion of Tokio's blocking pool (#3111).
pub(crate) static SEMANTIC_QUERY_SEMAPHORE: std::sync::LazyLock<Arc<tokio::sync::Semaphore>> =
    std::sync::LazyLock::new(|| {
        Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_SEMANTIC_QUERIES))
    });

/// Counter of currently active queries that have timed out and are still running in the background.
pub(crate) static STALLED_QUERIES_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Executes a read query on a thread-safe snapshot of `RustEngine`.
///
/// Holds the engine lock only briefly (<10 µs) to activate session overlays and take
/// an `Analysis` snapshot. The query then runs on a blocking thread pool worker without
/// retaining the shared engine lock, bounded by `SEMANTIC_QUERY_TIMEOUT`.
///
/// Bounded by `SEMANTIC_QUERY_SEMAPHORE` so outstanding blocking queries cannot accumulate
/// or exhaust Tokio's blocking pool. If timed-out queries accumulate and threaten capacity,
/// the engine is cooperatively recycled to purge stuck workers while active queries transparently
/// retry on fresh snapshots (#3109, #3111).
pub(crate) async fn execute_bounded_query<T, F>(
    engine_lock: &Arc<tokio::sync::Mutex<prod_code_engine_rust::RustEngine>>,
    session_id: u64,
    file_path: &Path,
    is_single_owner: bool,
    query_fn: F,
) -> anyhow::Result<T>
where
    T: Send + 'static,
    F: Fn(&prod_code_engine_rust::RustEngineSnapshot) -> anyhow::Result<T> + Clone + Send + 'static,
{
    let deadline = Instant::now() + SEMANTIC_QUERY_TIMEOUT;
    let mut retries = 0;

    let now = Instant::now();
    if now >= deadline {
        return Err(anyhow::anyhow!(
            "query timed out after {}s",
            SEMANTIC_QUERY_TIMEOUT.as_secs()
        ));
    }
    let remaining = deadline - now;

    let permit =
        match tokio::time::timeout(remaining, SEMANTIC_QUERY_SEMAPHORE.clone().acquire_owned())
            .await
        {
            Ok(Ok(permit)) => permit,
            Ok(Err(_closed)) => anyhow::bail!("semantic query semaphore closed"),
            Err(_elapsed) => {
                if STALLED_QUERIES_COUNT.load(Ordering::Relaxed) > 0 {
                    let engine_for_recycle = Arc::clone(engine_lock);
                    tokio::task::spawn(async move {
                        if let Ok(mut engine) =
                            tokio::time::timeout(Duration::from_secs(2), engine_for_recycle.lock())
                                .await
                        {
                            engine.trigger_cancellation();
                        }
                    });
                }
                anyhow::bail!("query timed out waiting for available execution slot");
            }
        };

    loop {
        let now = Instant::now();
        if now >= deadline {
            return Err(anyhow::anyhow!(
                "query timed out after {}s",
                SEMANTIC_QUERY_TIMEOUT.as_secs()
            ));
        }
        let remaining = deadline - now;

        let snapshot = match tokio::time::timeout(remaining, engine_lock.lock()).await {
            Ok(mut engine) => {
                if !is_single_owner
                    && engine.has_session_overlays()
                    && let Err(e) = engine.activate_session(session_id)
                {
                    tracing::warn!(error = %e, session = session_id, "session view activation failed");
                }
                if let Err(e) = engine.ensure_fresh_for_path(file_path) {
                    tracing::warn!(error = %e, file = %file_path.display(), "failed to ensure engine freshness");
                }
                if file_path.is_dir() {
                    engine.snapshot_for(file_path)
                } else {
                    engine.snapshot_for_path(file_path)
                }
            }
            Err(_elapsed) => {
                return Err(anyhow::anyhow!(
                    "query timed out after {}s waiting for engine lock",
                    SEMANTIC_QUERY_TIMEOUT.as_secs()
                ));
            }
        };

        let q_fn = query_fn.clone();
        let mut query_task = Box::pin(tokio::task::spawn_blocking(move || {
            let panic_res =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| q_fn(&snapshot)));
            match panic_res {
                Ok(res) => res,
                Err(panic_payload) => {
                    let msg = panic_message(panic_payload);
                    Err(anyhow::anyhow!("analyzer panic: {msg}"))
                }
            }
        }));

        let now = Instant::now();
        if now >= deadline {
            let stalled = STALLED_QUERIES_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
            let engine_for_recycle = Arc::clone(engine_lock);
            tokio::task::spawn(async move {
                let _held_permit = permit;
                if stalled >= (MAX_CONCURRENT_SEMANTIC_QUERIES / 2) {
                    tracing::warn!(
                        stalled,
                        "Stalled semantic queries reached threshold; recycling engine to reclaim blocking pool workers"
                    );
                    if let Ok(mut engine) =
                        tokio::time::timeout(Duration::from_secs(2), engine_for_recycle.lock())
                            .await
                    {
                        engine.trigger_cancellation();
                    }
                }
                let _ = query_task.await;
                STALLED_QUERIES_COUNT.fetch_sub(1, Ordering::Relaxed);
            });
            return Err(anyhow::anyhow!(
                "query timed out after {}s",
                SEMANTIC_QUERY_TIMEOUT.as_secs()
            ));
        }
        let remaining = deadline - now;

        match tokio::time::timeout(remaining, query_task.as_mut()).await {
            Ok(Ok(Ok(val))) => return Ok(val),
            Ok(Ok(Err(err))) => {
                if prod_code_engine_rust::is_salsa_cancelled(&err)
                    && retries < MAX_CANCELLATION_RETRIES
                {
                    let backoff = Duration::from_millis(5 * (retries + 1) as u64);
                    if Instant::now() + backoff < deadline {
                        retries += 1;
                        tracing::debug!(
                            session = session_id,
                            file = %file_path.display(),
                            attempt = retries,
                            "Salsa query was cancelled by concurrent mutation; retrying with fresh snapshot"
                        );
                        tokio::time::sleep(backoff).await;
                        continue;
                    }
                }
                return Err(err);
            }
            Ok(Err(join_err)) => {
                return Err(anyhow::anyhow!("native query task failed: {join_err}"));
            }
            Err(_elapsed) => {
                // The query timed out. Retain the permit while the detached task continues,
                // so that running timed-out tasks count against the concurrency bound and
                // cannot accumulate beyond MAX_CONCURRENT_SEMANTIC_QUERIES (#3111).
                let stalled = STALLED_QUERIES_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
                let engine_for_recycle = Arc::clone(engine_lock);
                tokio::task::spawn(async move {
                    let _held_permit = permit;
                    if stalled >= (MAX_CONCURRENT_SEMANTIC_QUERIES / 2) {
                        tracing::warn!(
                            stalled,
                            "Stalled semantic queries reached threshold; recycling engine to reclaim blocking pool workers"
                        );
                        if let Ok(mut engine) =
                            tokio::time::timeout(Duration::from_secs(2), engine_for_recycle.lock())
                                .await
                        {
                            engine.trigger_cancellation();
                        }
                    }
                    let _ = query_task.await;
                    STALLED_QUERIES_COUNT.fetch_sub(1, Ordering::Relaxed);
                });
                return Err(anyhow::anyhow!(
                    "query timed out after {}s",
                    SEMANTIC_QUERY_TIMEOUT.as_secs()
                ));
            }
        }
    }
}
