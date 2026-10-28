/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Result, anyhow};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use url::Url;

use super::budget::{expired as budget_expired, within_deadline};
use super::diff::{Change, Hunk};
use super::incoming::note;
use super::pool::SessionPool;
use super::signatures::check_adjusted_signature;
use super::symbols::{collect_functions, is_source_file};
use super::test_cmd::file_language;
use super::types::{Gap, Symbol};

pub(super) struct ChangedScan {
    pub(super) changed: Vec<Symbol>,
    pub(super) unattributed: Vec<String>,
    pub(super) adjusted_signatures: Vec<(Symbol, String, String, u32, u32)>,
    pub(super) incomplete: Vec<Gap>,
    pub(super) timed_out: bool,
}

pub(super) async fn scan_changed_files(
    root: &Path,
    language: &str,
    base: Option<&str>,
    changes: &BTreeMap<String, Change>,
    session_pool: &mut SessionPool<'_>,
    started_at: tokio::time::Instant,
    deadline: tokio::time::Instant,
) -> Result<ChangedScan> {
    let mut changed = Vec::new();
    let mut unattributed = Vec::new();
    let mut adjusted_signatures = Vec::new();
    let mut incomplete = Vec::new();
    let mut timed_out = false;

    for (file, change) in changes {
        if budget_expired(started_at, deadline, &mut incomplete) {
            timed_out = true;
            break;
        }
        if !is_source_file(file) {
            unattributed.push(file.clone());
            continue;
        }
        let file_hunks = match change {
            Change::Hunks(hunks) => hunks,
            Change::Unknown(error) => {
                note(
                    &mut incomplete,
                    Gap::Diff {
                        file: file.clone(),
                        error: error.clone(),
                    },
                );
                continue;
            }
        };
        let abs: PathBuf = root.join(file);
        if !abs.exists() {
            note(&mut incomplete, Gap::Deleted { file: file.clone() });
            continue;
        }
        if file_hunks.is_empty() {
            continue;
        }
        let text = match std::fs::read_to_string(&abs) {
            Ok(text) => text,
            Err(e) => {
                note(
                    &mut incomplete,
                    Gap::Symbols {
                        file: file.clone(),
                        error: format!("it cannot be read: {e}"),
                    },
                );
                continue;
            }
        };
        let file_lang = file_language(root, file).unwrap_or(language);
        let session = match within_deadline(deadline, session_pool.session_for_file(&abs)).await {
            Ok(Ok(session)) => session,
            Ok(Err(e)) => {
                note(
                    &mut incomplete,
                    Gap::Symbols {
                        file: file.clone(),
                        error: format!("{e:#}"),
                    },
                );
                continue;
            }
            Err(_) => {
                budget_expired(started_at, deadline, &mut incomplete);
                timed_out = true;
                break;
            }
        };
        let uri = Url::from_file_path(&abs)
            .map_err(|_| anyhow!("bad path {file}"))?
            .to_string();
        let symbol_result = match within_deadline(
            deadline,
            session.query(
                &abs,
                "textDocument/documentSymbol",
                serde_json::json!({ "textDocument": { "uri": uri } }),
            ),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => {
                budget_expired(started_at, deadline, &mut incomplete);
                timed_out = true;
                break;
            }
        };
        let symbols = match symbol_result {
            Ok(serde_json::Value::Array(symbols)) => symbols,
            result => {
                let error = match result {
                    Ok(serde_json::Value::Null) => {
                        "textDocument/documentSymbol answered null, so its functions are unknown"
                            .to_string()
                    }
                    Ok(other) => super::incoming::unreadable("textDocument/documentSymbol", &other),
                    Err(e) => format!("{e:#}"),
                };
                note(
                    &mut incomplete,
                    Gap::Symbols {
                        file: file.clone(),
                        error,
                    },
                );
                continue;
            }
        };
        let mut functions = Vec::new();
        if let Err(error) = collect_functions(&symbols, Some(&text), &mut functions) {
            note(
                &mut incomplete,
                Gap::Symbols {
                    file: file.clone(),
                    error,
                },
            );
            continue;
        }
        for (name, start, end, line, column) in &functions {
            if file_hunks
                .iter()
                .any(|hunk: &Hunk| hunk.touches(*start, *end))
            {
                let symbol = Symbol {
                    name: name.clone(),
                    file: file.clone(),
                    line: *line,
                    col: *column,
                };
                if !changed.contains(&symbol) {
                    changed.push(symbol.clone());
                }
                if let Some(adjusted) = check_adjusted_signature(
                    root, file, base, file_hunks, &text, name, file_lang, *line, &symbol,
                ) {
                    adjusted_signatures.push(adjusted);
                }
            }
        }
        let spans: Vec<(u32, u32)> = functions
            .iter()
            .map(|function| (function.1, function.2))
            .collect();
        let lines: Vec<&str> = text.lines().collect();
        if file_hunks.iter().any(|hunk| !hunk.inside(&spans, &lines)) {
            unattributed.push(file.clone());
        }
    }

    Ok(ChangedScan {
        changed,
        unattributed,
        adjusted_signatures,
        incomplete,
        timed_out,
    })
}
