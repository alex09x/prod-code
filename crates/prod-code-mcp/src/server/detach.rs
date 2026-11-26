/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Long commands outlive the MCP client's limit for one call (#2089).
//!
//! A host gives one tool call a few minutes (300 s is common) whatever `timeout_secs` says, then
//! cancels it, and the run's result was lost with it. A command tool (`code_exec`, `code_test`,
//! …) that is still running when its detach window ends is answered with a run ID instead; the
//! run goes on in this process, and `code_run_result` returns its result once it has one.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::watch;
use tokio::time::Instant;

/// The tools whose runs are detached when long: they run a command on a node.
pub(crate) const DETACHABLE: &[&str] = &[
    "code_exec",
    "code_test",
    "code_check",
    "code_lint",
    "code_benchmarks",
    "code_coverage",
    "code_diagnose_failure",
];

/// How long a call waits for its run before it answers with the run ID: under the 300 s many
/// hosts allow one call, with room for the answer to reach them.
pub(crate) const DEFAULT_DETACH_AFTER: Duration = Duration::from_secs(240);
/// The longest a `code_run_result` call waits, for the same reason.
pub(crate) const MAX_RESULT_WAIT: Duration = Duration::from_secs(240);
/// How long a finished run's result is kept for a `code_run_result` that has not come yet.
const KEEP_FINISHED: Duration = Duration::from_secs(3600);

/// A run's result: the `tools/call` result object, as it would have been answered.
type Outcome = Option<Arc<Value>>;

struct Run {
    tool: String,
    started: Instant,
    finished: Option<Instant>,
    outcome: watch::Receiver<Outcome>,
}

#[derive(Default)]
pub(crate) struct Runs {
    next: u64,
    runs: HashMap<String, Run>,
}

static RUNS: LazyLock<Mutex<Runs>> = LazyLock::new(Mutex::default);

fn runs() -> std::sync::MutexGuard<'static, Runs> {
    RUNS.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The detach window for a call of `tool` with `args`, or `None` when it is not detached: not a
/// command tool, `detach_after_secs: 0`, or `PROD_CODE_DETACH_SECS=0`.
pub(crate) fn detach_after(tool: &str, args: &Value) -> Option<Duration> {
    if !DETACHABLE.contains(&tool) {
        return None;
    }
    let secs = args
        .get("detach_after_secs")
        .and_then(Value::as_u64)
        .or_else(|| {
            std::env::var("PROD_CODE_DETACH_SECS")
                .ok()
                .and_then(|value| value.trim().parse().ok())
        });
    match secs {
        Some(0) => None,
        Some(secs) => Some(Duration::from_secs(secs)),
        None => Some(DEFAULT_DETACH_AFTER),
    }
}

/// Runs `call` (which yields the `tools/call` result object) and waits `after` for it. Its
/// result when it finishes in time; otherwise the answer that names the run, which keeps going.
pub(crate) async fn run_detachable<F>(tool: &str, after: Duration, call: F) -> Value
where
    F: Future<Output = Value> + Send + 'static,
{
    let (send, mut outcome) = watch::channel::<Outcome>(None);
    let id = {
        let mut runs = runs();
        runs.prune();
        runs.next += 1;
        let id = format!("run-{}", runs.next);
        runs.runs.insert(
            id.clone(),
            Run {
                tool: tool.to_string(),
                started: Instant::now(),
                finished: None,
                outcome: outcome.clone(),
            },
        );
        id
    };
    let finished_id = id.clone();
    tokio::spawn(async move {
        let value = call.await;
        if let Some(run) = runs().runs.get_mut(&finished_id) {
            run.finished = Some(Instant::now());
        }
        let _ = send.send(Some(Arc::new(value)));
    });
    match tokio::time::timeout(after, outcome.wait_for(Option::is_some)).await {
        Ok(Ok(done)) => {
            let value = done.as_deref().cloned().unwrap_or(Value::Null);
            runs().runs.remove(&id);
            value
        }
        _ => text_result(
            &format!(
                "{tool} is still running after {}s, as run `{id}`; it goes on and its result is \
                 kept. Call code_run_result {{\"run\": \"{id}\"}} for it (each call waits up to \
                 {}s). The MCP client ends a single call after a few minutes, whatever \
                 timeout_secs says, so a long run is answered this way.",
                after.as_secs(),
                MAX_RESULT_WAIT.as_secs()
            ),
            false,
        ),
    }
}

/// `code_run_result {run, wait_secs}`: the run's result once it has one (then forgotten), or
/// how long it has been running.
pub(crate) async fn run_result(args: &Value) -> Value {
    let Some(id) = args.get("run").and_then(Value::as_str) else {
        return text_result(
            "code_run_result needs `run`: the ID a long call answered with",
            true,
        );
    };
    let wait = args
        .get("wait_secs")
        .and_then(Value::as_u64)
        .map(Duration::from_secs)
        .unwrap_or(MAX_RESULT_WAIT)
        .min(MAX_RESULT_WAIT);
    let found = {
        let mut runs = runs();
        runs.prune();
        runs.runs
            .get(id)
            .map(|run| (run.tool.clone(), run.started, run.outcome.clone()))
    };
    let Some((tool, started, mut outcome)) = found else {
        let known: Vec<String> = runs().runs.keys().cloned().collect();
        return text_result(
            &format!(
                "no run `{id}` in this prod-code MCP server: its result was already returned, it \
                 finished over an hour ago, or the server restarted. Runs it holds: {}",
                if known.is_empty() {
                    "none".to_string()
                } else {
                    known.join(", ")
                }
            ),
            true,
        );
    };
    match tokio::time::timeout(wait, outcome.wait_for(Option::is_some)).await {
        Ok(Ok(done)) => {
            let value = done.as_deref().cloned().unwrap_or(Value::Null);
            runs().runs.remove(id);
            value
        }
        _ => text_result(
            &format!(
                "{tool} run `{id}` is still running, {}s after it started; call code_run_result \
                 again.",
                started.elapsed().as_secs()
            ),
            false,
        ),
    }
}

impl Runs {
    /// Forgets results kept longer than [`KEEP_FINISHED`]. A run still going is kept.
    fn prune(&mut self) {
        self.runs.retain(|_, run| {
            run.finished
                .is_none_or(|finished| finished.elapsed() < KEEP_FINISHED)
        });
    }
}

fn text_result(text: &str, is_error: bool) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "isError": is_error })
}

#[cfg(test)]
#[path = "detach_tests.rs"]
mod tests;
