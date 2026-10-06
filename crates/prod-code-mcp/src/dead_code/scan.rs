/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::reachability_scan::run_reachability_scan;
use super::syntax::{collect, extensions, is_exported, is_test_path, reference_count};
use super::types::{DeadCodeOptions, DeadCodeReport, DeadItem, Unverified};
use crate::session::LspSession;
use anyhow::{Result, anyhow};
use std::net::SocketAddr;
use std::path::Path;

/// Scans the checkout at `root` (placed on `remote`) for unreferenced symbols.
pub async fn find_dead_code(
    remote: SocketAddr,
    root: &Path,
    include_exported: bool,
    max_files: usize,
) -> Result<DeadCodeReport> {
    find_dead_code_opts(
        remote,
        root,
        DeadCodeOptions {
            include_exported,
            max_files,
            reachability: false,
        },
    )
    .await
}

/// Scans the checkout at `root` (placed on `remote`) with configurable options
/// (including whole-program graph reachability analysis from entry points).
pub async fn find_dead_code_opts(
    remote: SocketAddr,
    root: &Path,
    options: DeadCodeOptions,
) -> Result<DeadCodeReport> {
    let language = crate::sync::expected_engine(root)
        .ok_or_else(|| anyhow!("no project manifest at {}", root.display()))?
        .to_string();
    let exts = extensions(&language);
    let mut files: Vec<String> = crate::sync::scan_workspace_files(root, None)?
        .into_iter()
        .map(|d| d.relative_path)
        .filter(|p| {
            Path::new(p)
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| exts.contains(&e))
        })
        .filter(|p| !is_test_path(&language, p))
        .collect();
    files.sort();
    let truncated = files.len() > options.max_files;
    files.truncate(options.max_files);

    let mut session = LspSession::open(remote, root, None).await?;
    let mut report = DeadCodeReport {
        language: language.clone(),
        files_scanned: 0,
        symbols_checked: 0,
        dead: Vec::new(),
        methods_unreferenced: Vec::new(),
        exported_unreferenced: 0,
        truncated,
        unverified: Vec::new(),
        reachability: None,
        unreachable_clusters: Vec::new(),
        root_entry_points: Vec::new(),
    };
    let whole_file = |file: &str, reason: String| Unverified {
        file: file.to_string(),
        name: None,
        line: 0,
        col: 0,
        reason,
    };

    if options.reachability {
        run_reachability_scan(&mut session, root, &files, &language, options, &mut report).await?;
    } else {
        // Single-pass reference counting scan
        for rel in &files {
            let abs = root.join(rel);
            let text = match std::fs::read_to_string(&abs) {
                Ok(text) => text,
                Err(e) => {
                    report
                        .unverified
                        .push(whole_file(rel, format!("it cannot be read: {e}")));
                    continue;
                }
            };
            let lines: Vec<&str> = text.lines().collect();
            let uri = session.uri_for(&abs)?;
            let symbols = match session
                .query(
                    &abs,
                    "textDocument/documentSymbol",
                    serde_json::json!({ "textDocument": { "uri": uri } }),
                )
                .await
            {
                Ok(serde_json::Value::Array(symbols)) => symbols,
                // The protocol's "no result": it does not say the file has no symbols.
                Ok(serde_json::Value::Null) => {
                    let reason =
                        "textDocument/documentSymbol answered null, so its symbols are unknown";
                    report.unverified.push(whole_file(rel, reason.to_string()));
                    continue;
                }
                Ok(other) => {
                    let reason = crate::impact::unreadable("textDocument/documentSymbol", &other);
                    report.unverified.push(whole_file(rel, reason));
                    continue;
                }
                Err(e) => {
                    report.unverified.push(whole_file(rel, format!("{e:#}")));
                    continue;
                }
            };
            let mut candidates = Vec::new();
            if let Err(reason) = collect(&symbols, &mut candidates) {
                report.unverified.push(whole_file(rel, reason));
                continue;
            }
            report.files_scanned += 1;
            for (name, kind, line, col) in candidates {
                let bare = name.split('(').next().unwrap_or(&name);
                if matches!(
                    bare,
                    "main" | "init" | "new" | "default" | "drop" | "fmt" | "eq" | "hash" | "clone"
                ) || bare.starts_with("test")
                    || bare.starts_with("Test")
                    || bare.starts_with("__")
                {
                    continue;
                }
                let source_line = lines.get(line as usize - 1).copied().unwrap_or("");
                let exported = is_exported(&language, bare, source_line);
                report.symbols_checked += 1;
                let refs = session
                    .query(
                        &abs,
                        "textDocument/references",
                        serde_json::json!({
                            "textDocument": { "uri": session.uri_for(&abs)? },
                            "position": { "line": line - 1, "character": col - 1 },
                            "context": { "includeDeclaration": false }
                        }),
                    )
                    .await;
                let count = match reference_count(refs) {
                    Ok(count) => count,
                    Err(reason) => {
                        report.unverified.push(Unverified {
                            file: rel.clone(),
                            name: Some(name),
                            line,
                            col,
                            reason,
                        });
                        continue;
                    }
                };
                if count == 0 {
                    let item = DeadItem {
                        name,
                        kind: kind.clone(),
                        file: rel.clone(),
                        line,
                        col,
                        exported,
                    };
                    // Rust inherent methods are checked like functions; trait-impl methods and
                    // methods in languages with interfaces/protocols go to the "maybe" bucket.
                    if kind == "trait-method" || (kind == "method" && language != "rust") {
                        report.methods_unreferenced.push(item);
                    } else if exported && !options.include_exported {
                        report.exported_unreferenced += 1;
                    } else {
                        report.dead.push(item);
                    }
                }
            }
        }
    }
    session.close().await;
    Ok(report)
}
