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

use crate::diagnostics::ident::{identifier_columns, mentions_identifier, rust_code_identifiers};
use crate::diagnostics::types::{DiagnosticsReport, DocDiagnostic};

/// Code of the warning prod-code synthesises for a line that still uses a removed symbol.
pub const STALE_REFERENCE: &str = "prod-code::stale-reference";

/// Explains, and where the analyzer stayed silent reports, uses of a symbol that the proposed
/// edits removed or renamed. `missing` pairs a symbol name with the file it disappeared from;
/// `sources` maps a report's file to the text its diagnostics were computed against.
///
/// An existing error or warning on such a line gets a note. A line with no diagnostic gets a
/// synthesised warning: rust-analyzer does not report a plain call to a function that no
/// longer exists (that is rustc's E0425), so without this the report would say "0 errors"
/// for a caller the edit just broke. The file the symbol vanished from is skipped: mentions
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
                "this line uses `{name}`, which the proposed edit to {from} removed or renamed; update the caller or keep the symbol"
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
                    let clean_name = name.strip_prefix("r#").unwrap_or(name);
                    if tok.name == clean_name {
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
                        "uses `{name}`, which the proposed edits remove or rename (the analyzer reports no error for a plain call to a missing function; run code_check to be sure)"
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
            for item in report.items.iter_mut() {
                if item.severity != "error" && item.severity != "warning" {
                    continue;
                }
                let Some(line) = lines.get(item.line.saturating_sub(1) as usize) else {
                    continue;
                };
                if let Some((name, from)) = relevant
                    .iter()
                    .find(|(name, _)| mentions_identifier(line, name))
                {
                    item.note = Some(note_for(name, from));
                    flagged.insert(item.line);
                }
            }
            for (idx, line) in lines.iter().enumerate() {
                let line_no = idx as u32 + 1;
                if flagged.contains(&line_no) {
                    continue;
                }
                for (name, from) in &relevant {
                    let mut matched = false;
                    for col in identifier_columns(line, name) {
                        if resolved.contains(&(report.file.clone(), line_no, col)) {
                            continue;
                        }
                        report.items.push(DocDiagnostic {
                            severity: "warning".to_string(),
                            code: Some(STALE_REFERENCE.to_string()),
                            message: format!(
                                "uses `{name}`, which the proposed edits remove or rename (the analyzer reports no error for a plain call to a missing function; run code_check to be sure)"
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
