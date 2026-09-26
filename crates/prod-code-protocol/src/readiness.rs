//! Whether a language server has finished loading and indexing, from what it says itself: the
//! work-done progress it begins and ends (`$/progress`), or, for a server that reports none, the
//! log line that says it is set up. Asked before that, clangd answered `workspace/symbol` with
//! nothing and then with the part of the index built so far, and basedpyright with nothing (#391).

use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How a server tells that it is ready to answer from its index.
#[derive(Clone, Copy, Debug, Default)]
pub enum ReadySignal {
    /// It reports its loading and indexing as work-done progress (gopls, clangd, sourcekit-lsp):
    /// ready when no work it began is still going.
    Progress,
    /// It reports no progress but logs a line when it is set up (basedpyright and pyright:
    /// `Found 2000 source files`); the function recognises that line.
    Log(fn(&str) -> bool),
    /// It holds a question until it can answer it (the native TypeScript server).
    HoldsQuestions,
    /// Nothing is known: its answers are taken as they come.
    #[default]
    Unknown,
}

/// The work a server is still doing when asked: what it calls it, how far it is, and for how
/// long it has been at it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Busy {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percentage: Option<u64>,
    /// Milliseconds since the work began.
    pub for_ms: u64,
}

impl Busy {
    /// "indexing (1234/5000, 25%) for 3 s", for a note under an answer.
    pub fn describe(&self) -> String {
        let mut detail = Vec::new();
        if let Some(message) = self.message.as_deref().filter(|m| !m.is_empty()) {
            detail.push(message.to_string());
        }
        if let Some(percentage) = self.percentage {
            detail.push(format!("{percentage}%"));
        }
        let detail = if detail.is_empty() {
            String::new()
        } else {
            format!(" ({})", detail.join(", "))
        };
        format!("{}{detail} for {} s", self.title, self.for_ms / 1000)
    }
}

/// The JSON-RPC member an engine adds to an answer given while its server was still busy, and
/// the notification the gateway turns it into for its client.
pub const BUSY_MEMBER: &str = "prodCodeIndexing";
pub const BUSY_NOTIFICATION: &str = "prod-code/indexing";

/// Requests a server answers from its index, which are empty or partial until it has indexed.
pub fn needs_index(method: &str) -> bool {
    matches!(
        method,
        "workspace/symbol"
            | "textDocument/references"
            | "textDocument/implementation"
            | "textDocument/rename"
            | "callHierarchy/incomingCalls"
            | "callHierarchy/outgoingCalls"
            | "typeHierarchy/supertypes"
            | "typeHierarchy/subtypes"
    )
}

/// How long a question answered from the index waits for the server to finish loading and
/// indexing before it is asked anyway, with a note of how far the server got.
pub const INDEX_WAIT: Duration = Duration::from_secs(30);

/// How long after `initialized` a server that reports progress may take to begin it: gopls and
/// clangd began within 0.15 s on a build node.
const SETTLE: Duration = Duration::from_millis(500);

/// How long a server that announces its readiness in its log is waited for at most: one that
/// never logs the line is not waited on forever.
const LOG_LIMIT: Duration = Duration::from_secs(120);

/// How often a wait looks again when nothing was reported: the settle and log windows end
/// without a message.
const RECHECK: Duration = Duration::from_millis(100);

struct Work {
    token: String,
    title: String,
    message: Option<String>,
    percentage: Option<u64>,
    began: Instant,
}

struct State {
    started: Instant,
    active: Vec<Work>,
    progress_seen: bool,
    logged_ready: bool,
}

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

/// A progress token as text, whether the server sent it as a string or a number.
fn token_text(token: &serde_json::Value) -> String {
    match token {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Whether a basedpyright or pyright log line says it has found its source files, after which
/// it holds a question until it can answer it.
pub fn pyright_found_sources(line: &str) -> bool {
    line == "No source files found."
        || (line.starts_with("Found ") && line.trim_end().ends_with(" source files"))
        || (line.starts_with("Found ") && line.trim_end().ends_with(" source file"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn progress(token: &str, value: serde_json::Value) -> serde_json::Value {
        json!({ "jsonrpc": "2.0", "method": "$/progress", "params": { "token": token, "value": value } })
    }

    #[tokio::test]
    async fn work_begun_and_not_ended_is_busy_until_it_ends() {
        let ready = Readiness::new(ReadySignal::Progress);
        assert_eq!(
            ready.busy().map(|b| b.title),
            Some("starting".to_string()),
            "a progress server is given a moment to begin"
        );
        ready.on_message(&json!({ "jsonrpc": "2.0", "id": 1, "method": "window/workDoneProgress/create", "params": { "token": "backgroundIndexProgress" } }));
        ready.on_message(&progress(
            "backgroundIndexProgress",
            json!({ "kind": "begin", "title": "indexing", "percentage": 0 }),
        ));
        ready.on_message(&progress(
            "backgroundIndexProgress",
            json!({ "kind": "report", "message": "1234/5000", "percentage": 25 }),
        ));
        let busy = ready.busy().expect("indexing");
        assert_eq!(
            (
                busy.title.as_str(),
                busy.message.as_deref(),
                busy.percentage
            ),
            ("indexing", Some("1234/5000"), Some(25))
        );
        assert!(
            busy.describe()
                .starts_with("indexing (1234/5000, 25%) for ")
        );
        ready.on_message(&progress(
            "backgroundIndexProgress",
            json!({ "kind": "end" }),
        ));
        assert_eq!(ready.busy(), None);
    }

    #[tokio::test]
    async fn a_wait_ends_when_the_work_ends_or_when_the_time_is_up() {
        let ready = std::sync::Arc::new(Readiness::new(ReadySignal::Progress));
        ready.on_message(&progress(
            "7",
            json!({ "kind": "begin", "title": "Setting up workspace", "message": "Loading packages..." }),
        ));
        let still = ready
            .wait(Duration::from_millis(150))
            .await
            .expect("still loading");
        assert_eq!(still.message.as_deref(), Some("Loading packages..."));

        let ending = std::sync::Arc::clone(&ready);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            ending.on_message(&progress(
                "7",
                json!({ "kind": "end", "message": "Finished loading packages." }),
            ));
        });
        let started = Instant::now();
        assert_eq!(ready.wait(Duration::from_secs(10)).await, None);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "woken by the end, not the limit"
        );
    }

    #[tokio::test]
    async fn work_created_but_not_yet_begun_is_busy() {
        let ready = Readiness::new(ReadySignal::Progress);
        ready.on_message(&json!({ "jsonrpc": "2.0", "id": 1, "method": "window/workDoneProgress/create", "params": { "token": 9 } }));
        assert_eq!(ready.busy().map(|b| b.title), Some("starting".to_string()));
        ready.on_message(&json!({ "jsonrpc": "2.0", "method": "$/progress", "params": { "token": 9, "value": { "kind": "begin", "title": "Setting up workspace" } } }));
        assert_eq!(
            ready.busy().map(|b| b.title),
            Some("Setting up workspace".to_string())
        );
        ready.on_message(&json!({ "jsonrpc": "2.0", "method": "$/progress", "params": { "token": 9, "value": { "kind": "end" } } }));
        assert_eq!(ready.busy(), None);
    }

    #[tokio::test]
    async fn a_numeric_token_and_an_end_without_a_begin_are_taken_in() {
        let ready = Readiness::new(ReadySignal::Progress);
        ready.on_message(&progress("x", json!({ "kind": "end" })));
        ready.on_message(&json!({ "jsonrpc": "2.0", "method": "$/progress", "params": { "token": 42, "value": { "kind": "begin", "title": "Loading" } } }));
        assert_eq!(ready.busy().map(|b| b.title), Some("Loading".to_string()));
        ready.on_message(&json!({ "jsonrpc": "2.0", "method": "$/progress", "params": { "token": 42, "value": { "kind": "end" } } }));
        assert_eq!(
            ready.busy(),
            None,
            "progress was seen, so no settle window either"
        );
    }

    #[tokio::test]
    async fn a_log_server_is_busy_until_its_line_and_others_never() {
        let pyright = Readiness::new(ReadySignal::Log(pyright_found_sources));
        assert!(pyright.known());
        assert_eq!(
            pyright.busy().and_then(|b| b.message),
            Some("setting up its project".to_string())
        );
        pyright.on_message(&json!({ "jsonrpc": "2.0", "method": "window/logMessage", "params": { "type": 4, "message": "Assuming Python version 3.10.12.final.0" } }));
        assert!(pyright.busy().is_some());
        pyright.on_message(&json!({ "jsonrpc": "2.0", "method": "window/logMessage", "params": { "type": 4, "message": "Found 20000 source files" } }));
        assert_eq!(pyright.busy(), None);

        let unknown = Readiness::new(ReadySignal::Unknown);
        assert!(!unknown.known());
        assert_eq!(unknown.busy(), None);
        assert_eq!(unknown.wait(Duration::from_secs(5)).await, None);
        assert!(Readiness::new(ReadySignal::HoldsQuestions).busy().is_none());
    }

    #[test]
    fn the_pyright_line_and_the_index_queries_are_recognised() {
        assert!(pyright_found_sources("Found 2000 source files"));
        assert!(pyright_found_sources("Found 1 source file"));
        assert!(pyright_found_sources("No source files found."));
        assert!(!pyright_found_sources("Found pyproject.toml"));
        assert!(needs_index("workspace/symbol") && needs_index("textDocument/references"));
        assert!(!needs_index("textDocument/hover") && !needs_index("textDocument/definition"));
    }

    #[test]
    fn busy_goes_over_the_wire_without_empty_members() {
        let busy = Busy {
            title: "indexing".to_string(),
            message: None,
            percentage: Some(40),
            for_ms: 3200,
        };
        let json = serde_json::to_value(&busy).unwrap();
        assert_eq!(
            json,
            json!({ "title": "indexing", "percentage": 40, "for_ms": 3200 })
        );
        assert_eq!(serde_json::from_value::<Busy>(json).unwrap(), busy);
        assert_eq!(busy.describe(), "indexing (40%) for 3 s");
    }
}
