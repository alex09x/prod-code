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

/// One deadline shared by all language groups, session setup and optional enrichment.
/// Each file gets at most 2s for its baseline, 1s per symbol query and 5s for diagnostics.
/// A large batch still returns within 90s, including when the analyzer never replies.
pub(super) struct ValidationBudget {
    pub(super) deadline: Instant,
    pub(super) diagnostic: Duration,
    pub(super) auxiliary: Duration,
}

impl ValidationBudget {
    pub(super) fn new() -> Self {
        Self {
            deadline: Instant::now() + Duration::from_secs(90),
            diagnostic: Duration::from_secs(5),
            auxiliary: Duration::from_secs(1),
        }
    }

    pub(super) async fn run<T>(
        &self,
        cap: Duration,
        stage: &str,
        future: impl Future<Output = Result<T>>,
    ) -> Result<T> {
        let deadline = self.deadline.min(Instant::now() + cap);
        if deadline <= Instant::now() {
            anyhow::bail!("validation batch budget exhausted before {stage}");
        }
        tokio::time::timeout_at(deadline, future)
            .await
            .map_err(|_| anyhow::anyhow!("validation budget exhausted during {stage}"))?
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
