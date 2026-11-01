/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::future::Future;
use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use serde_json::Value;
use tokio::time::Instant;

use crate::diagnostics::parse::invalid_report;
use crate::diagnostics::types::DiagnosticsReport;
use crate::session::LspSession;

use super::progress::ValidationProgress;

/// One deadline shared by all language groups, session setup and optional enrichment, with a
/// cap on each step. A refactoring's own check waits for a cold analyzer as long as a session
/// can take to open: with 5 s per file and 10 s to open, every check against a cold
/// rust-analyzer came back "not validated" and refactorings refused to write. The interactive
/// tool answers before the MCP client gives up on it instead (#1172, #1176).
#[derive(Clone)]
pub(crate) struct ValidationBudget {
    pub(super) deadline: Instant,
    pub(super) diagnostic: Duration,
    pub(super) auxiliary: Duration,
    pub(super) session: Duration,
    pub(super) baseline: Duration,
    pub(super) progress: Option<ValidationProgress>,
}

impl ValidationBudget {
    /// For checks a refactoring runs before it writes: bounded, but patient with a cold engine.
    pub(crate) fn new() -> Self {
        Self {
            deadline: Instant::now() + Duration::from_secs(600),
            diagnostic: Duration::from_secs(60),
            auxiliary: Duration::from_secs(30),
            session: Duration::from_secs(180),
            baseline: Duration::from_secs(600),
            progress: None,
        }
    }

    /// Refactoring validation budget tailored to the edited files: bounds Swift `sourcekit-lsp`
    /// validation so a stalled or unindexed Swift target reports promptly (#1780).
    pub(crate) fn for_edits(edits: &[(std::path::PathBuf, String)]) -> Self {
        let is_swift = !edits.is_empty()
            && edits
                .iter()
                .all(|(p, _)| p.extension().and_then(|e| e.to_str()) == Some("swift"));
        if is_swift {
            Self {
                deadline: Instant::now() + Duration::from_secs(15),
                diagnostic: Duration::from_secs(5),
                auxiliary: Duration::from_secs(4),
                session: Duration::from_secs(10),
                baseline: Duration::from_secs(5),
                progress: None,
            }
        } else {
            Self::new()
        }
    }

    /// For `code_validate_edits`: answers within 120 s, below the MCP client's 150 s limit, and
    /// names any file it could not check in that time.
    pub(crate) fn interactive() -> Self {
        Self {
            deadline: Instant::now() + Duration::from_secs(120),
            baseline: Duration::from_secs(30),
            ..Self::new()
        }
    }

    /// Optional before-state diagnostics and symbols share a small part of an
    /// interactive request, leaving time to check the actual proposal.
    pub(super) fn baseline(&self) -> Self {
        Self {
            deadline: self.deadline.min(Instant::now() + self.baseline),
            ..self.clone()
        }
    }

    pub(super) async fn run<T>(
        &self,
        cap: Duration,
        stage: &str,
        future: impl Future<Output = Result<T>>,
    ) -> Result<T> {
        if let Some(progress) = &self.progress {
            progress.phase(stage);
        }
        let deadline = self.deadline.min(Instant::now() + cap);
        if deadline <= Instant::now() {
            anyhow::bail!("validation batch budget exhausted before {stage}");
        }
        tokio::time::timeout_at(deadline, future)
            .await
            .map_err(|_| anyhow::anyhow!("validation budget exhausted during {stage}"))?
    }

    pub(super) fn completed(&self, count: usize) {
        if let Some(progress) = &self.progress {
            progress.completed(count);
        }
    }

    pub(super) async fn query(
        &self,
        session: &mut LspSession,
        file: &Path,
        method: &str,
        mut params: Value,
        cap: Duration,
    ) -> Result<Value> {
        let deadline = self.deadline.min(Instant::now() + cap);
        self.run(cap, method, async {
            loop {
                // Bound the gateway's blocking worker as well as the client's wait.
                params["prodCodeTimeoutMs"] = serde_json::json!(
                    deadline
                        .saturating_duration_since(Instant::now())
                        .as_millis()
                        .max(1) as u64
                );
                match session.query(file, method, params.clone()).await {
                    Err(err) if is_cancelled(&err) => {
                        // Every request asks for a fresh snapshot after all proposal writes.
                        // Older gateways may exhaust their own small retry count first.
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                    result => return result,
                }
            }
        })
        .await
    }
}

fn is_cancelled(err: &anyhow::Error) -> bool {
    let message = err.to_string().to_ascii_lowercase();
    message.contains("cancelled") || message.contains("canceled")
}

pub(super) fn incomplete_report(file: &str, reason: &str) -> DiagnosticsReport {
    let mut report = invalid_report(file, reason);
    report.items[0].code = Some("prod-code-validation-incomplete".into());
    report.items[0].message = format!(
        "{file} was not validated: {reason}. Other files retain their results; retry this file or request an explicit compiler check"
    );
    report
}
