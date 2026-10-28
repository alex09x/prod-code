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

use crate::diagnostics::ident::{identifier_columns, rust_code_identifiers};
use crate::diagnostics::types::{DiagnosticsReport, DocDiagnostic};

/// Code of the warning prod-code synthesises for a possible stale reference.
pub const STALE_REFERENCE: &str = "prod-code::stale-reference";

/// Reports unresolved code tokens that may refer to symbols removed or renamed by the edit.
/// `missing` pairs a symbol name with the file it disappeared from; `sources` maps a report's
/// file to the text its diagnostics were computed against. Name matches do not prove identity.
///
/// An existing error or warning on such a line gets a note. A line with no diagnostic gets a
/// synthesised warning: rust-analyzer does not report a plain call to a function that no
/// longer exists (that is rustc's E0425), so without this the report would say "0 errors"
/// for a potentially broken caller. The file the symbol vanished from is skipped: mentions
/// of the old name there are its own doc comments.
pub fn annotate_missing_symbols(
    reports: &mut [DiagnosticsReport],
    sources: &HashMap<String, String>,
    missing: &[(String, String)],
    resolved: &BTreeSet<(String, u32, u32)>,
) {
    if missing.is_empty() {
        return;
    }
    for report in reports.iter_mut() {
        let Some(text) = sources.get(&report.file) else {
            continue;
        };
        let relevant: Vec<&(String, String)> = missing
            .iter()
            .filter(|(_, from)| *from != report.file)
            .collect();
        if relevant.is_empty() {
            continue;
        }
        let lines: Vec<&str> = text.lines().collect();
        let note_for = |name: &str, from: &str| {
            format!(
                "this line mentions `{name}`, which may refer to a symbol removed or renamed by the proposed edit to {from}; check the reference"
            )
        };
        let mut flagged: BTreeSet<u32> = BTreeSet::new();
        let is_rust = report.file.ends_with(".rs");

        if is_rust {
            let tokens = rust_code_identifiers(text);
            let mut line_matches: HashMap<u32, Vec<(&str, &str, u32)>> = HashMap::new();
            for tok in &tokens {
                if resolved.contains(&(report.file.clone(), tok.line, tok.col)) {
                    continue;
                }
                for (name, from) in &relevant {
                    if super::ident::names_removed(tok, name) {
                        line_matches.entry(tok.line).or_default().push((
                            name.as_str(),
                            from.as_str(),
                            tok.col,
                        ));
                        break;
                    }
                }
            }

            for item in report.items.iter_mut() {
                if item.severity != "error" && item.severity != "warning" {
                    continue;
                }
                if let Some(matches) = line_matches.get(&item.line)
                    && let Some(&(name, from, _)) = matches.first()
                {
                    item.note = Some(note_for(name, from));
                    flagged.insert(item.line);
                }
            }

            for (idx, _) in lines.iter().enumerate() {
                let line_no = idx as u32 + 1;
                if flagged.contains(&line_no) {
                    continue;
                }
                let Some(matches) = line_matches.get(&line_no) else {
                    continue;
                };
                let Some(&(name, from, col)) = matches.first() else {
                    continue;
                };
                report.items.push(DocDiagnostic {
                    severity: "warning".to_string(),
                    code: Some(STALE_REFERENCE.to_string()),
                    message: format!(
                        "`{name}` may refer to a symbol removed or renamed by the proposed edits; check the reference and run code_check"
                    ),
                    line: line_no,
                    col,
                    source: Some("prod-code".to_string()),
                    note: Some(note_for(name, from)),
                    end: None,
                });
                report.warnings += 1;
            }
        } else {
            use crate::tools::references::across::{mask_executable_code, masked_code_string};
            let path = std::path::Path::new(&report.file);
            let code = masked_code_string(text, &mask_executable_code(text, path));
            let code_lines: Vec<&str> = code.lines().collect();
            for item in report.items.iter_mut() {
                if item.severity != "error" && item.severity != "warning" {
                    continue;
                }
                let Some(line) = lines.get(item.line.saturating_sub(1) as usize) else {
                    continue;
                };
                let Some(code_line) = code_lines.get(item.line.saturating_sub(1) as usize) else {
                    continue;
                };
                if let Some((name, from)) = relevant.iter().find(|(name, _)| {
                    executable_columns(line, code_line, name)
                        .into_iter()
                        .any(|col| !resolved.contains(&(report.file.clone(), item.line, col)))
                }) {
                    item.note = Some(note_for(name, from));
                    flagged.insert(item.line);
                }
            }
            for (idx, line) in lines.iter().enumerate() {
                let line_no = idx as u32 + 1;
                if flagged.contains(&line_no) {
                    continue;
                }
                let Some(code_line) = code_lines.get(idx) else {
                    continue;
                };
                for (name, from) in &relevant {
                    let mut matched = false;
                    for col in executable_columns(line, code_line, name) {
                        if resolved.contains(&(report.file.clone(), line_no, col)) {
                            continue;
                        }
                        report.items.push(DocDiagnostic {
                            severity: "warning".to_string(),
                            code: Some(STALE_REFERENCE.to_string()),
                            message: format!(
                                "`{name}` may refer to a symbol removed or renamed by the proposed edits; check the reference and run code_check"
                            ),
                            line: line_no,
                            col,
                            source: Some("prod-code".to_string()),
                            note: Some(note_for(name, from)),
                            end: None,
                        });
                        report.warnings += 1;
                        flagged.insert(line_no);
                        matched = true;
                        break;
                    }
                    if matched {
                        break;
                    }
                }
            }
        }
        report.items.sort_by_key(|d| (d.line, d.col));
    }
}

fn executable_columns(line: &str, code_line: &str, name: &str) -> Vec<u32> {
    let identifiers = identifier_columns(line, name);
    line.match_indices(name)
        .filter_map(|(byte, _)| {
            let col = line[..byte].encode_utf16().count() as u32 + 1;
            (identifiers.contains(&col) && code_line.get(byte..byte + name.len()) == Some(name))
                .then_some(col)
        })
        .collect()
}

/// Notes on each stale-reference warning that the definition check behind it did not finish:
/// its deadline passed or a request failed, so the use may be of another symbol of that name
/// that was never asked about (#1465).
pub fn mark_unfinished_resolution(reports: &mut [DiagnosticsReport]) {
    for item in reports
        .iter_mut()
        .flat_map(|r| r.items.iter_mut())
        .filter(|d| d.code.as_deref() == Some(STALE_REFERENCE))
    {
        let note = item.note.take().unwrap_or_default();
        item.note = Some(format!(
            "{note} (the analyzer was not asked about every use before its deadline or a failed request, so this may name another symbol of that name)"
        ).trim_start().to_string());
    }
}
