//! In-memory diagnostics for a document (roadmap 7.7): what the analyzer thinks of a file,
//! or of a proposed replacement text, without a build and without writing anything.

use crate::session::LspSession;
use anyhow::Result;
use serde::Serialize;
use std::net::SocketAddr;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct DocDiagnostic {
    pub severity: String,
    pub code: Option<String>,
    pub message: String,
    pub line: u32,
    pub col: u32,
    pub source: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticsReport {
    pub file: String,
    pub errors: usize,
    pub warnings: usize,
    pub items: Vec<DocDiagnostic>,
}

impl DiagnosticsReport {
    pub fn ok(&self) -> bool {
        self.errors == 0
    }

    pub fn render(&self) -> String {
        let mut out = format!(
            "{}: {} error(s), {} warning(s)\n",
            self.file, self.errors, self.warnings
        );
        for d in &self.items {
            out.push_str(&format!(
                "  {}: {}{} ({}:{}:{})\n",
                d.severity,
                d.message.lines().next().unwrap_or(""),
                d.code
                    .as_deref()
                    .map(|c| format!(" [{c}]"))
                    .unwrap_or_default(),
                self.file,
                d.line,
                d.col
            ));
        }
        out
    }
}

fn parse_items(file: &str, result: &serde_json::Value) -> DiagnosticsReport {
    let items: Vec<DocDiagnostic> = result
        .get("items")
        .and_then(|i| i.as_array())
        .map(|arr| {
            arr.iter()
                .map(|d| {
                    let start = d.get("range").and_then(|r| r.get("start"));
                    let severity = match d.get("severity").and_then(|s| s.as_u64()) {
                        Some(1) => "error",
                        Some(2) => "warning",
                        Some(3) => "info",
                        Some(4) => "hint",
                        _ => "error",
                    };
                    DocDiagnostic {
                        severity: severity.to_string(),
                        code: d.get("code").map(|c| match c {
                            serde_json::Value::String(s) => s.clone(),
                            other => other.to_string(),
                        }),
                        message: d
                            .get("message")
                            .and_then(|m| m.as_str())
                            .unwrap_or("")
                            .to_string(),
                        line: start
                            .and_then(|s| s.get("line"))
                            .and_then(|l| l.as_u64())
                            .unwrap_or(0) as u32
                            + 1,
                        col: start
                            .and_then(|s| s.get("character"))
                            .and_then(|c| c.as_u64())
                            .unwrap_or(0) as u32
                            + 1,
                        source: d.get("source").and_then(|s| s.as_str()).map(String::from),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    DiagnosticsReport {
        file: file.to_string(),
        errors: items.iter().filter(|d| d.severity == "error").count(),
        warnings: items.iter().filter(|d| d.severity == "warning").count(),
        items,
    }
}

/// Diagnostics of `file` as it is on disk.
pub async fn diagnostics(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
) -> Result<DiagnosticsReport> {
    let mut session = LspSession::open(remote, root, Some(file)).await?;
    let uri = session.uri_for(file)?;
    let result = session
        .query(
            file,
            "textDocument/diagnostic",
            serde_json::json!({ "textDocument": { "uri": uri } }),
        )
        .await?;
    session.close().await;
    Ok(parse_items(&display(root, file), &result))
}

/// Diagnostics of `file` as if its content were `new_text`; nothing is written.
pub async fn validate_text(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    new_text: &str,
) -> Result<DiagnosticsReport> {
    let mut session = LspSession::open(remote, root, Some(file)).await?;
    let uri = session.uri_for(file)?;
    let result = session
        .query_with_text(
            file,
            new_text,
            "textDocument/diagnostic",
            serde_json::json!({ "textDocument": { "uri": uri } }),
        )
        .await?;
    session.close().await;
    Ok(parse_items(&display(root, file), &result))
}

/// Validates several proposed file contents together, the way a multi-file refactor must be
/// judged: every file is opened with its new text in one session (a private overlay on the
/// gateway), then diagnostics are pulled for each of them and for `also_check` (unchanged
/// files that may break, typically callers of an edited symbol). An edit in one file is
/// therefore checked against the proposed state of the others, not against the checkout.
/// Nothing is written anywhere.
pub async fn validate_texts(
    remote: SocketAddr,
    root: &Path,
    edits: &[(std::path::PathBuf, String)],
    also_check: &[std::path::PathBuf],
) -> Result<Vec<DiagnosticsReport>> {
    let hint = edits
        .first()
        .map(|(file, _)| file.as_path())
        .or_else(|| also_check.first().map(|p| p.as_path()));
    let mut session = LspSession::open(remote, root, hint).await?;
    let mut uris = Vec::with_capacity(edits.len());
    for (file, text) in edits {
        uris.push((file.clone(), session.open_text(file, text).await?));
    }
    let mut reports = Vec::with_capacity(edits.len() + also_check.len());
    for (file, uri) in &uris {
        let result = session
            .request(
                "textDocument/diagnostic",
                serde_json::json!({ "textDocument": { "uri": uri } }),
            )
            .await?;
        reports.push(parse_items(&display(root, file), &result));
    }
    for file in also_check {
        let uri = session.uri_for(file)?;
        let result = session
            .query(
                file,
                "textDocument/diagnostic",
                serde_json::json!({ "textDocument": { "uri": uri } }),
            )
            .await?;
        reports.push(parse_items(&display(root, file), &result));
    }
    session.close().await;
    Ok(reports)
}

fn display(root: &Path, file: &Path) -> String {
    let abs = if file.is_absolute() {
        file.to_path_buf()
    } else {
        root.join(file)
    };
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let abs = std::fs::canonicalize(&abs).unwrap_or(abs);
    abs.strip_prefix(&root)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| abs.to_string_lossy().into_owned())
}
