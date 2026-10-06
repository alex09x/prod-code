/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use crate::diagnostics::ident::{identifier_columns, rust_code_identifiers};
use crate::diagnostics::types::{DiagnosticsReport, DocDiagnostic};
use crate::session::LspSession;

/// A re-export reference in a checked caller proves that a public `pub use` is used, even
/// when rust-analyzer tags the import as unused. Suppress only when the references request at the
/// re-export token returns a location in an unchanged checked caller; unresolved or unreferenced
/// imports remain diagnostics.
pub async fn suppress_used_public_reexport_warnings(
    session: &mut LspSession,
    root: &Path,
    edit_count: usize,
    reports: &mut [DiagnosticsReport],
    sources: &HashMap<String, String>,
) {
    let mut suppress = BTreeSet::new();
    let candidates: Vec<_> = reports
        .iter()
        .take(edit_count)
        .enumerate()
        .flat_map(|(report_index, report)| {
            let Some(text) = sources.get(&report.file) else {
                return Vec::new();
            };
            report
                .items
                .iter()
                .enumerate()
                .filter_map(move |(item_index, item)| {
                    if item.severity != "warning"
                        || item.source.as_deref() != Some("rust-analyzer")
                        || item.code.as_deref() != Some("unused_imports")
                    {
                        return None;
                    }
                    let col = public_reexport_token(text, item)?;
                    Some((
                        report_index,
                        item_index,
                        report.file.clone(),
                        item.line,
                        col,
                    ))
                })
                .collect::<Vec<_>>()
        })
        .collect();

    for (report_index, item_index, file, line, col) in candidates {
        let Ok(uri) = session.uri_for(&root.join(&file)) else {
            continue;
        };
        let caller_uris: Vec<String> = reports
            .iter()
            .skip(edit_count)
            .filter(|caller| caller.file.ends_with(".rs") && caller.file != file)
            .filter_map(|caller| session.uri_for(&root.join(&caller.file)).ok())
            .collect();
        if caller_uris.is_empty() {
            continue;
        }
        let references = session
            .request(
                "textDocument/references",
                serde_json::json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": line - 1, "character": col - 1 },
                    "context": { "includeDeclaration": false }
                }),
            )
            .await;
        let used = references
            .as_ref()
            .ok()
            .and_then(serde_json::Value::as_array)
            .is_some_and(|locations| {
                locations.iter().any(|location| {
                    let Some(uri) = location.get("uri").and_then(serde_json::Value::as_str) else {
                        return false;
                    };
                    let valid_range = location
                        .get("range")
                        .and_then(|range| range.get("start"))
                        .is_some_and(|start| {
                            start
                                .get("line")
                                .and_then(serde_json::Value::as_u64)
                                .is_some()
                                && start
                                    .get("character")
                                    .and_then(serde_json::Value::as_u64)
                                    .is_some()
                        });
                    valid_range && caller_uris.iter().any(|caller| caller == uri)
                })
            });
        if used {
            suppress.insert((report_index, item_index));
        }
    }

    for (report_index, report) in reports.iter_mut().enumerate() {
        if !suppress.iter().any(|(index, _)| *index == report_index) {
            continue;
        }
        let items = std::mem::take(&mut report.items);
        report.items = items
            .into_iter()
            .enumerate()
            .filter_map(|(item_index, item)| {
                (!suppress.contains(&(report_index, item_index))).then_some(item)
            })
            .collect();
        report.errors = report
            .items
            .iter()
            .filter(|item| item.severity == "error")
            .count();
        report.warnings = report
            .items
            .iter()
            .filter(|item| item.severity == "warning")
            .count();
    }
}

pub fn public_reexport_token(text: &str, diagnostic: &DocDiagnostic) -> Option<u32> {
    let line_number = diagnostic.line;
    let line = text.lines().nth(line_number.checked_sub(1)? as usize)?;
    let statement = line
        .trim_start()
        .strip_prefix("pub use ")?
        .split(';')
        .next()?
        .trim();
    let local_names: Vec<&str> = if let Some(open) = statement.find('{') {
        let close = statement.rfind('}')?;
        if close < open || !statement[close + 1..].trim().is_empty() {
            return None;
        }
        statement[open + 1..close]
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(|item| {
                item.rsplit_once(" as ")
                    .map(|(_, alias)| alias.trim())
                    .unwrap_or_else(|| item.rsplit("::").next().unwrap_or(item).trim())
            })
            .collect()
    } else {
        if statement.chars().any(|ch| matches!(ch, '}' | '*')) {
            return None;
        }
        vec![
            statement
                .rsplit_once(" as ")
                .map(|(_, alias)| alias.trim())
                .unwrap_or_else(|| statement.rsplit("::").next().unwrap_or(statement).trim()),
        ]
    };
    let names: BTreeSet<&str> = local_names
        .into_iter()
        .map(|name| name.strip_prefix("r#").unwrap_or(name))
        .collect();
    let diagnostic_start = (diagnostic.line, diagnostic.col);
    let diagnostic_end = diagnostic.end?;
    let token = rust_code_identifiers(text)
        .into_iter()
        .filter(|token| {
            token.line == line_number
                && names.contains(token.name.as_str())
                && diagnostic_start <= (token.line, token.col)
                && (token.line, token.col) < diagnostic_end
        })
        .max_by_key(|token| token.col)?;
    Some(token.col)
}

/// A declaration removed from one file may still resolve in the complete proposal: a move,
/// re-export or unrelated same-named binding is not a broken caller. Ask while the whole
/// overlay is open. Missing or malformed semantic evidence keeps the conservative warning.
pub async fn resolved_tokens(
    session: &mut LspSession,
    root: &Path,
    reports: &[DiagnosticsReport],
    sources: &HashMap<String, String>,
    missing: &[(String, String)],
) -> BTreeSet<(String, u32, u32)> {
    let mut resolved = BTreeSet::new();
    if missing.is_empty() {
        return resolved;
    }
    for report in reports.iter() {
        let Some(text) = sources.get(&report.file) else {
            continue;
        };
        let names: BTreeSet<&str> = missing
            .iter()
            .filter(|(_, from)| from != &report.file)
            .map(|(name, _)| name.strip_prefix("r#").unwrap_or(name))
            .collect();
        if names.is_empty() {
            continue;
        }
        let Ok(uri) = session.uri_for(&root.join(&report.file)) else {
            continue;
        };
        let is_rust = report.file.ends_with(".rs");
        if is_rust {
            for token in rust_code_identifiers(text) {
                if !names.contains(token.name.as_str()) {
                    continue;
                }
                let answer = session
                    .request(
                        "textDocument/definition",
                        serde_json::json!({
                            "textDocument": {"uri": uri},
                            "position": {"line": token.line - 1, "character": token.col - 1}
                        }),
                    )
                    .await;
                if answer.as_ref().is_ok_and(has_definition) {
                    resolved.insert((report.file.clone(), token.line, token.col));
                }
            }
        } else {
            for (idx, line) in text.lines().enumerate() {
                let line_no = idx as u32 + 1;
                for &name in &names {
                    for col in identifier_columns(line, name) {
                        let answer = session
                            .request(
                                "textDocument/definition",
                                serde_json::json!({
                                    "textDocument": {"uri": uri},
                                    "position": {"line": line_no - 1, "character": col - 1}
                                }),
                            )
                            .await;
                        if answer.as_ref().is_ok_and(has_definition) {
                            resolved.insert((report.file.clone(), line_no, col));
                        }
                    }
                }
            }
        }
    }
    resolved
}

pub fn has_definition(value: &serde_json::Value) -> bool {
    fn location(value: &serde_json::Value) -> bool {
        if value.get("error").is_some() {
            return false;
        }
        let (uri, range) = if value.get("targetUri").is_some() {
            (value.get("targetUri"), value.get("targetSelectionRange"))
        } else {
            (value.get("uri"), value.get("range"))
        };
        let Some(uri) = uri.and_then(|u| u.as_str()) else {
            return false;
        };
        if url::Url::parse(uri)
            .ok()
            .and_then(|u| u.to_file_path().ok())
            .is_none()
        {
            return false;
        }
        let Some(range) = range else { return false };
        let position = |key: &str| -> Option<(u32, u32)> {
            let p = range.get(key)?;
            Some((
                u32::try_from(p.get("line")?.as_u64()?).ok()?,
                u32::try_from(p.get("character")?.as_u64()?).ok()?,
            ))
        };
        matches!((position("start"), position("end")), (Some(start), Some(end)) if start <= end)
    }
    match value {
        serde_json::Value::Array(items) => !items.is_empty() && items.iter().all(location),
        serde_json::Value::Object(_) => location(value),
        _ => false,
    }
}

pub fn display(root: &Path, file: &Path) -> String {
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
