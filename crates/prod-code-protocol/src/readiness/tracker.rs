/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::{Busy, LOG_LIMIT, RECHECK, ReadySignal, SETTLE, State, Work, token_text};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// What one server has said about its loading and indexing.
pub struct Readiness {
    signal: ReadySignal,
    state: Mutex<State>,
    changed: tokio::sync::Notify,
}

impl Readiness {
    pub fn new(signal: ReadySignal) -> Self {
        Self {
            signal,
            state: Mutex::new(State {
                started: Instant::now(),
                active: Vec::new(),
                progress_seen: false,
                logged_ready: false,
            }),
            changed: tokio::sync::Notify::new(),
        }
    }

    /// Whether the server's readiness is known, so that its answers after [`Self::wait`] are
    /// final: an empty one means there is nothing, not that it has not looked yet.
    pub fn known(&self) -> bool {
        !matches!(self.signal, ReadySignal::Unknown)
    }

    /// Marks the moment `initialized` went to the server, from which the settle and log windows
    /// count.
    pub fn started(&self) {
        self.lock().started = Instant::now();
    }

    /// Takes in one message from the server: `$/progress` begins, reports and ends work,
    /// `window/workDoneProgress/create` announces it, and a log line may say it is set up.
    pub fn on_message(&self, message: &serde_json::Value) {
        let method = message.get("method").and_then(|m| m.as_str());
        let mut state = self.lock();
        match method {
            // Work is on its way: the server begins it once the client has answered, and a
            // question slipped into that gap met a gopls that had not started loading (#391).
            Some("window/workDoneProgress/create") => {
                state.progress_seen = true;
                if let Some(token) = message.pointer("/params/token").map(token_text)
                    && !state.active.iter().any(|w| w.token == token)
                {
                    state.active.push(Work {
                        token,
                        title: "starting".to_string(),
                        message: Some("about to begin its work".to_string()),
                        percentage: None,
                        began: Instant::now(),
                    });
                }
            }
            Some("$/progress") => {
                let Some(token) = message.pointer("/params/token").map(token_text) else {
                    return;
                };
                let Some(value) = message.pointer("/params/value") else {
                    return;
                };
                let text = |key: &str| value.get(key).and_then(|v| v.as_str()).map(String::from);
                let percentage = value.get("percentage").and_then(|p| p.as_u64());
                state.progress_seen = true;
                match value.get("kind").and_then(|k| k.as_str()) {
                    Some("begin") => {
                        state.active.retain(|w| w.token != token);
                        state.active.push(Work {
                            token,
                            title: text("title").unwrap_or_else(|| "working".to_string()),
                            message: text("message"),
                            percentage,
                            began: Instant::now(),
                        });
                    }
                    Some("report") => {
                        if let Some(work) = state.active.iter_mut().find(|w| w.token == token) {
                            if let Some(message) = text("message") {
                                work.message = Some(message);
                            }
                            if percentage.is_some() {
                                work.percentage = percentage;
                            }
                        }
                    }
                    Some("end") => {
                        state.active.retain(|w| w.token != token);
                        drop(state);
                        self.changed.notify_waiters();
                    }
                    _ => {}
                }
            }
            Some("window/logMessage") => {
                if let ReadySignal::Log(ready) = self.signal
                    && message
                        .pointer("/params/message")
                        .and_then(|m| m.as_str())
                        .is_some_and(ready)
                {
                    state.logged_ready = true;
                    drop(state);
                    self.changed.notify_waiters();
                }
            }
            _ => {}
        }
    }

    /// The work the server is still doing, or `None` when it is ready (or nothing is known).
    pub fn busy(&self) -> Option<Busy> {
        let state = self.lock();
        if let Some(work) = state.active.first() {
            return Some(Busy {
                title: work.title.clone(),
                message: work.message.clone(),
                percentage: work.percentage,
                for_ms: work.began.elapsed().as_millis() as u64,
            });
        }
        let starting = |detail: &str| {
            Some(Busy {
                title: "starting".to_string(),
                message: Some(detail.to_string()),
                percentage: None,
                for_ms: state.started.elapsed().as_millis() as u64,
            })
        };
        match self.signal {
            ReadySignal::Log(_) if !state.logged_ready && state.started.elapsed() < LOG_LIMIT => {
                starting("setting up its project")
            }
            ReadySignal::Progress if !state.progress_seen && state.started.elapsed() < SETTLE => {
                starting("about to load its project")
            }
            _ => None,
        }
    }

    /// Waits until the server is ready, at most `max`; returns the work it is still doing when
    /// the time is up, or `None` when it became ready.
    pub async fn wait(&self, max: Duration) -> Option<Busy> {
        let deadline = Instant::now() + max;
        loop {
            let notified = self.changed.notified();
            let busy = self.busy()?;
            let now = Instant::now();
            if now >= deadline {
                return Some(busy);
            }
            let _ = tokio::time::timeout(RECHECK.min(deadline - now), notified).await;
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}
