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

#[path = "bounded_query_control.rs"]
mod control;
use control::CancelOnDrop;

pub(crate) const SEMANTIC_QUERY_TIMEOUT: Duration = Duration::from_secs(60);

/// Validation hints may shorten, but never enlarge, the server's query budget.
pub(crate) fn semantic_query_budget(params: &serde_json::Value) -> Duration {
    params
        .get("prodCodeTimeoutMs")
        .and_then(serde_json::Value::as_u64)
        .map(|ms| Duration::from_millis(ms.max(1)).min(SEMANTIC_QUERY_TIMEOUT))
        .unwrap_or(SEMANTIC_QUERY_TIMEOUT)
}

pub(crate) const MAX_CONCURRENT_SEMANTIC_QUERIES: usize = 16;
pub(crate) static SEMANTIC_QUERY_SEMAPHORE: std::sync::LazyLock<Arc<tokio::sync::Semaphore>> =
    std::sync::LazyLock::new(|| {
        Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_SEMANTIC_QUERIES))
    });
pub(crate) static STALLED_QUERIES_COUNT: AtomicUsize = AtomicUsize::new(0);

fn timeout_label(timeout: Duration) -> String {
    if timeout.subsec_millis() == 0 {
        format!("{}s", timeout.as_secs())
    } else {
        format!("{}ms", timeout.as_millis())
    }
}

#[cfg(test)]
#[path = "validation_query_tests.rs"]
mod validation_query_tests;

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
    execute_bounded_query_with_timeout(
        engine_lock,
        session_id,
        file_path,
        is_single_owner,
        SEMANTIC_QUERY_TIMEOUT,
        query_fn,
    )
    .await
}

/// Preparation, execution and cancellation retries share one deadline. The worker retains
/// its capacity permit until it exits, but its Salsa handle is revocable independently.
pub(crate) async fn execute_bounded_query_with_timeout<T, F>(
    engine_lock: &Arc<tokio::sync::Mutex<prod_code_engine_rust::RustEngine>>,
    session_id: u64,
    file_path: &Path,
    is_single_owner: bool,
    timeout: Duration,
    query_fn: F,
) -> anyhow::Result<T>
where
    T: Send + 'static,
    F: Fn(&prod_code_engine_rust::RustEngineSnapshot) -> anyhow::Result<T> + Clone + Send + 'static,
{
    let timeout = timeout.min(SEMANTIC_QUERY_TIMEOUT);
    let deadline = Instant::now() + timeout;
    let timed_out = || anyhow::anyhow!("query timed out after {}", timeout_label(timeout));
    let permit = tokio::time::timeout(timeout, SEMANTIC_QUERY_SEMAPHORE.clone().acquire_owned())
        .await
        .map_err(|_| anyhow::anyhow!("query timed out waiting for available execution slot"))?
        .map_err(|_| anyhow::anyhow!("semantic query semaphore closed"))?;
    let permit = Arc::new(permit);
    let mut retries = 0;

    loop {
        if Instant::now() >= deadline {
            return Err(timed_out());
        }
        // Bound lock acquisition asynchronously, then move all synchronous preparation
        // (including session writes and manifest reloads) off the runtime thread.
        let engine = tokio::time::timeout(
            deadline.saturating_duration_since(Instant::now()),
            Arc::clone(engine_lock).lock_owned(),
        )
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "query timed out after {} waiting for engine lock",
                timeout_label(timeout)
            )
        })?;
        let mut cancellation = CancelOnDrop::new(engine_lock);
        let control = Arc::clone(&cancellation.control);
        let path = file_path.to_path_buf();
        let q_fn = query_fn.clone();
        let held_permit = Arc::clone(&permit);
        let mut query_task = tokio::task::spawn_blocking(move || {
            let _permit = held_permit;
            let panic_res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut engine = engine;
                if Instant::now() >= deadline {
                    anyhow::bail!("query deadline expired before preparation");
                }
                if !is_single_owner
                    && engine.has_session_overlays()
                    && let Err(e) = engine.activate_session(session_id)
                {
                    tracing::warn!(error = %e, session = session_id, "session view activation failed");
                }
                if let Err(e) = engine.ensure_fresh_for_path(&path) {
                    tracing::warn!(error = %e, file = %path.display(), "failed to ensure engine freshness");
                }
                let snapshot = if path.is_dir() {
                    engine.snapshot_for(&path)
                } else {
                    engine.snapshot_for_path(&path)
                };
                control.install(&snapshot)?;
                drop(engine);
                if Instant::now() >= deadline {
                    anyhow::bail!("query deadline expired before execution");
                }
                q_fn(&snapshot)
            }));
            match panic_res {
                Ok(res) => res,
                Err(payload) => Err(anyhow::anyhow!(
                    "analyzer panic: {}",
                    panic_message(payload)
                )),
            }
        });

        let outcome = tokio::time::timeout(
            deadline.saturating_duration_since(Instant::now()),
            &mut query_task,
        )
        .await;
        if outcome.is_ok() {
            cancellation.disarm();
        }
        match outcome {
            Ok(Ok(Ok(val))) => return Ok(val),
            Ok(Ok(Err(err))) if prod_code_engine_rust::is_salsa_cancelled(&err) => {
                let backoff = Duration::from_millis(5 * (retries + 1).min(10) as u64);
                if Instant::now() + backoff >= deadline {
                    return Err(timed_out());
                }
                retries += 1;
                tokio::time::sleep(backoff).await;
            }
            Ok(Ok(Err(err))) => return Err(err),
            Ok(Err(err)) => return Err(anyhow::anyhow!("native query task failed: {err}")),
            Err(_) => {
                // Revoke now, even when q_fn is blocked on unrelated work.
                drop(cancellation);
                STALLED_QUERIES_COUNT.fetch_add(1, Ordering::Relaxed);
                tokio::task::spawn(async move {
                    let _ = query_task.await;
                    STALLED_QUERIES_COUNT.fetch_sub(1, Ordering::Relaxed);
                });
                return Err(timed_out());
            }
        }
    }
}
