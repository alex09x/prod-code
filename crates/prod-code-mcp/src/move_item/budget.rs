/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result};
use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::time::Instant;

/// A move has one interactive budget, rather than fresh 180/300-second session/query
/// allowances for every declaration, reference, baseline and proposed diagnostic (#1142).
pub(super) struct MoveBudget {
    deadline: Instant,
}

impl MoveBudget {
    pub(super) fn new() -> Self {
        Self {
            deadline: Instant::now() + Duration::from_secs(60),
        }
    }

    pub(super) async fn run<T>(
        &self,
        stage: &str,
        future: impl Future<Output = Result<T>>,
    ) -> Result<T> {
        tokio::time::timeout_at(self.deadline, future)
            .await
            .map_err(|_| anyhow::anyhow!(
                "move exceeded its 60-second planning budget while {stage}; nothing was written. \
                 Run `prod-code cluster` to inspect analyzer capacity, then retry on a ready node"
            ))?
            .with_context(|| format!("move failed while {stage}; nothing was written"))
    }

    pub(super) async fn validate(
        &self,
        remote: SocketAddr,
        root: &Path,
        edits: &[(PathBuf, String)],
        callers: &[PathBuf],
    ) -> Result<Vec<String>> {
        let reports = self
            .run(
                "validating proposed files",
                crate::diagnostics::validate_texts(remote, root, edits, callers),
            )
            .await?;
        Ok(reports
            .iter()
            .flat_map(|r| r.items.iter().map(move |d| (r.file.clone(), d)))
            .filter(|(_, d)| d.severity == "error")
            .map(|(f, d)| {
                format!(
                    "{}{} ({f}:{}:{})",
                    d.message.lines().next().unwrap_or(""),
                    d.code
                        .as_deref()
                        .map(|c| format!(" [{c}]"))
                        .unwrap_or_default(),
                    d.line,
                    d.col
                )
            })
            .collect())
    }
}

pub(crate) async fn document_symbols(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
) -> Result<serde_json::Value> {
    let uri = url::Url::from_file_path(file)
        .map_err(|_| anyhow::anyhow!("invalid path {:?}", file))?
        .to_string();
    crate::tools::execute_lsp_query(
        remote,
        root,
        file,
        "textDocument/documentSymbol",
        serde_json::json!({ "textDocument": { "uri": uri } }),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn move_budget_is_shared_by_all_planning_phases() {
        let budget = MoveBudget {
            deadline: Instant::now() + Duration::from_millis(30),
        };
        budget
            .run("finding references", async {
                tokio::time::sleep(Duration::from_millis(20)).await;
                Ok(())
            })
            .await
            .unwrap();
        let started = Instant::now();
        let error = budget
            .run(
                "validating proposed files",
                std::future::pending::<Result<()>>(),
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(started.elapsed() < Duration::from_millis(100));
        assert!(error.contains("validating proposed files"), "{error}");
        assert!(error.contains("nothing was written"), "{error}");
    }
}
