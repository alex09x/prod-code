//! In-memory diagnostics for a document (roadmap 7.7): what the analyzer thinks of a file,
//! or of a proposed replacement text, without a build and without writing anything.

use crate::session::LspSession;
use anyhow::Result;
use serde::Serialize;
use std::collections::{BTreeSet, HashMap};
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
    /// Extra explanation added by prod-code (for example that the failing line uses a symbol
    /// the proposed edits removed or renamed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticsReport {
    pub file: String,
    pub errors: usize,
    pub warnings: usize,
    pub items: Vec<DocDiagnostic>,
    /// Diagnostics the file already had on disk, before the edit under review: the same
    /// severity, code and message on a line with the same text. They are not the edit's, so
    /// they are neither in `items` nor counted in `errors` and `warnings`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub preexisting: Vec<DocDiagnostic>,
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
        if !self.preexisting.is_empty() {
            out.push_str(&format!(
                "  ({} diagnostic(s) the file already had before this edit are not counted: {})\n",
                self.preexisting.len(),
                preexisting_summary(&self.preexisting)
            ));
        }
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
            if let Some(note) = &d.note {
                out.push_str(&format!("    note: {note}\n"));
            }
        }
        out
    }
}

/// The distinct messages among `items`, most frequent first, each with how often it occurs.
fn preexisting_summary(items: &[DocDiagnostic]) -> String {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for d in items {
        let message = format!(
            "{}{}",
            d.message.lines().next().unwrap_or(""),
            d.code
                .as_deref()
                .map(|c| format!(" [{c}]"))
                .unwrap_or_default()
        );
        match counts.iter_mut().find(|(m, _)| *m == message) {
            Some((_, n)) => *n += 1,
            None => counts.push((message, 1)),
        }
    }
    counts.sort_by_key(|a| std::cmp::Reverse(a.1));
    counts
        .iter()
        .map(|(m, n)| format!("{n}× {m}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// What makes two diagnostics the same one across an edit: lines move, so not the position,
/// but the text of the line it is on.
fn identity(d: &DocDiagnostic, text: &str) -> (String, Option<String>, String, String) {
    let line = text
        .lines()
        .nth(d.line.saturating_sub(1) as usize)
        .unwrap_or("")
        .trim()
        .to_string();
    (d.severity.clone(), d.code.clone(), d.message.clone(), line)
}

/// Moves from `report.items` to `report.preexisting` every diagnostic that `before` — the same
/// file's diagnostics against `before_text`, the text on disk — already had. Each diagnostic
/// before the edit accounts for at most one after it, so a second copy of an old error on a new
/// line is still the edit's.
fn set_aside_preexisting(
    report: &mut DiagnosticsReport,
    text: &str,
    before: &DiagnosticsReport,
    before_text: &str,
) {
    let mut old: HashMap<(String, Option<String>, String, String), usize> = HashMap::new();
    for d in &before.items {
        *old.entry(identity(d, before_text)).or_default() += 1;
    }
    let items = std::mem::take(&mut report.items);
    for d in items {
        match old.get_mut(&identity(&d, text)) {
            Some(n) if *n > 0 => {
                *n -= 1;
                report.preexisting.push(d);
            }
            _ => report.items.push(d),
        }
    }
    report.errors = report
        .items
        .iter()
        .filter(|d| d.severity == "error")
        .count();
    report.warnings = report
        .items
        .iter()
        .filter(|d| d.severity == "warning")
        .count();
}

/// Every symbol name in a `textDocument/documentSymbol` result (flat or hierarchical).
fn symbol_names(result: &serde_json::Value) -> BTreeSet<String> {
    fn walk(value: &serde_json::Value, out: &mut BTreeSet<String>) {
        match value {
            serde_json::Value::Array(items) => items.iter().for_each(|i| walk(i, out)),
            serde_json::Value::Object(map) => {
                if let Some(name) = map.get("name").and_then(|n| n.as_str()) {
                    out.insert(name.to_string());
                }
                if let Some(children) = map.get("children") {
                    walk(children, out);
                }
            }
            _ => {}
        }
    }
    let mut out = BTreeSet::new();
    walk(result, &mut out);
    out
}

/// Whether `line` mentions `name` as a whole identifier.
fn mentions_identifier(line: &str, name: &str) -> bool {
    let bytes = line.as_bytes();
    let mut from = 0;
    while let Some(pos) = line[from..].find(name) {
        let start = from + pos;
        let end = start + name.len();
        let before_ok =
            start == 0 || !(bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_');
        let after_ok =
            end >= bytes.len() || !(bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_');
        if before_ok && after_ok {
            return true;
        }
        from = end;
    }
    false
}

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
fn annotate_missing_symbols(
    reports: &mut [DiagnosticsReport],
    sources: &HashMap<String, String>,
    missing: &[(String, String)],
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
            let Some((name, from)) = relevant
                .iter()
                .find(|(name, _)| mentions_identifier(line, name))
            else {
                continue;
            };
            let col = line.find(name.as_str()).unwrap_or(0) as u32 + 1;
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
            });
            report.warnings += 1;
        }
        report.items.sort_by_key(|d| (d.line, d.col));
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
                        note: None,
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
                // `inactive-code` marks the branch of a `#[cfg]` pair that is off on the
                // node (`#[cfg(not(unix))]` on Linux). It is correct and says nothing about
                // the edit under review, so agents never see it.
                .filter(|d| d.code.as_deref() != Some("inactive-code"))
                .collect()
        })
        .unwrap_or_default();
    DiagnosticsReport {
        file: file.to_string(),
        errors: items.iter().filter(|d| d.severity == "error").count(),
        warnings: items.iter().filter(|d| d.severity == "warning").count(),
        items,
        preexisting: Vec::new(),
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
    let shown = display(root, file);
    let before = {
        let mut checkout = LspSession::open(remote, root, Some(file)).await?;
        let before = on_disk(&mut checkout, root, file, &shown).await;
        checkout.close().await;
        before
    };
    let mut session = LspSession::open_for_validation(remote, root, Some(file)).await?;
    let uri = session.uri_for(file)?;
    let params = serde_json::json!({ "textDocument": { "uri": uri } });
    let result = session
        .query_with_text(file, new_text, "textDocument/diagnostic", params)
        .await?;
    session.close().await;
    let mut report = parse_items(&shown, &result);
    if let Some((before, before_text)) = before {
        set_aside_preexisting(&mut report, new_text, &before, &before_text);
    }
    Ok(report)
}

/// The diagnostics of `file` as it is on disk, and that text, or `None` for a file that does
/// not exist yet.
///
/// Asked of the main engine, in a session of its own, never of the validation engine: the main
/// engine holds the checkout's state warm, so this costs what any diagnostics query costs. The
/// validation engine is left holding only proposals, and one that repeats the last proposal —
/// the same dry run asked twice — finds everything it needs still computed (#73).
async fn on_disk(
    session: &mut LspSession,
    root: &Path,
    file: &Path,
    shown: &str,
) -> Option<(DiagnosticsReport, String)> {
    let abs = if file.is_absolute() {
        file.to_path_buf()
    } else {
        root.join(file)
    };
    let text = std::fs::read_to_string(&abs).ok()?;
    let uri = session.uri_for(file).ok()?;
    let result = session
        .query(
            file,
            "textDocument/diagnostic",
            serde_json::json!({ "textDocument": { "uri": uri } }),
        )
        .await
        .ok()?;
    Some((parse_items(shown, &result), text))
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
    // What every file says as it is on disk: an error the checkout already has is not the
    // edit's, and a report that counts it refuses every edit to that file.
    let mut baselines: HashMap<String, (DiagnosticsReport, String)> = HashMap::new();
    {
        let mut checkout = LspSession::open(remote, root, hint).await?;
        for file in edits.iter().map(|(f, _)| f).chain(also_check) {
            let shown = display(root, file);
            if let Some(before) = on_disk(&mut checkout, root, file, &shown).await {
                baselines.insert(shown, before);
            }
        }
        checkout.close().await;
    }
    let mut session = LspSession::open_for_validation(remote, root, hint).await?;
    let mut uris = Vec::with_capacity(edits.len());
    // Symbols that the proposed texts remove or rename, with the file they vanish from: an
    // error on a line that still uses one of them gets an explaining note, because the
    // analyzer itself reports such a call as "type annotations needed" or "cannot find".
    let mut missing: Vec<(String, String)> = Vec::new();
    let mut sources: HashMap<String, String> = HashMap::new();
    for (file, text) in edits {
        let uri = session.uri_for(file)?;
        let symbols_params = serde_json::json!({ "textDocument": { "uri": uri } });
        let before = if root.join(file).is_file() || file.is_file() {
            session
                .query(file, "textDocument/documentSymbol", symbols_params.clone())
                .await
                .map(|r| symbol_names(&r))
                .unwrap_or_default()
        } else {
            BTreeSet::new()
        };
        let uri = session.open_text(file, text).await?;
        let after = session
            .request("textDocument/documentSymbol", symbols_params)
            .await
            .map(|r| symbol_names(&r))
            .unwrap_or_default();
        let shown = display(root, file);
        for name in before.difference(&after) {
            missing.push((name.clone(), shown.clone()));
        }
        sources.insert(shown, text.clone());
        uris.push((file.clone(), uri));
    }
    let mut reports = Vec::with_capacity(edits.len() + also_check.len());
    for (file, uri) in &uris {
        let result = session
            .request(
                "textDocument/diagnostic",
                serde_json::json!({ "textDocument": { "uri": uri } }),
            )
            .await?;
        let shown = display(root, file);
        let mut report = parse_items(&shown, &result);
        if let (Some((before, before_text)), Some(text)) =
            (baselines.get(&shown), sources.get(&shown))
        {
            set_aside_preexisting(&mut report, text, before, before_text);
        }
        reports.push(report);
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
        let shown = display(root, file);
        let abs = if file.is_absolute() {
            file.clone()
        } else {
            root.join(file)
        };
        let mut report = parse_items(&shown, &result);
        if let Ok(text) = std::fs::read_to_string(&abs) {
            if let Some((before, before_text)) = baselines.get(&shown) {
                set_aside_preexisting(&mut report, &text, before, before_text);
            }
            sources.insert(shown.clone(), text);
        }
        reports.push(report);
    }
    session.close().await;
    annotate_missing_symbols(&mut reports, &sources, &missing);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inactive_code_hints_are_dropped_and_counts_ignore_them() {
        let result = serde_json::json!({ "items": [
            { "range": { "start": { "line": 3, "character": 4 } }, "severity": 4,
              "code": "inactive-code", "message": "code is inactive due to #[cfg] directives: unix is enabled" },
            { "range": { "start": { "line": 10, "character": 8 } }, "severity": 1,
              "code": "E0425", "message": "cannot find value `x` in this scope" },
            { "range": { "start": { "line": 12, "character": 1 } }, "severity": 4,
              "code": "unused_variables", "message": "unused variable" }
        ]});
        let report = parse_items("src/lib.rs", &result);
        assert_eq!(report.errors, 1);
        assert_eq!(report.warnings, 0);
        assert_eq!(report.items.len(), 2, "{:?}", report.items);
        assert!(
            report
                .items
                .iter()
                .all(|d| d.code.as_deref() != Some("inactive-code"))
        );
        assert_eq!(report.items[0].line, 11);
        assert_eq!(report.items[0].col, 9);
    }

    #[test]
    fn symbol_names_walks_flat_and_hierarchical_results() {
        let flat = serde_json::json!([
            { "name": "shared_target_dir", "kind": 12 },
            { "name": "Tracked", "kind": 23 }
        ]);
        assert_eq!(
            symbol_names(&flat).into_iter().collect::<Vec<_>>(),
            vec!["Tracked".to_string(), "shared_target_dir".to_string()]
        );
        let tree = serde_json::json!([
            { "name": "Outer", "kind": 23, "children": [ { "name": "inner", "kind": 6 } ] }
        ]);
        assert!(symbol_names(&tree).contains("inner"));
    }

    #[test]
    fn mentions_identifier_matches_whole_words_only() {
        assert!(mentions_identifier(
            "    let d = workspace::shared_target_dir(&ws);",
            "shared_target_dir"
        ));
        assert!(!mentions_identifier(
            "    let d = shared_target_dir_renamed(&ws);",
            "shared_target_dir"
        ));
        assert!(!mentions_identifier(
            "    let x = my_shared_target_dir;",
            "shared_target_dir"
        ));
    }

    fn diagnostic(severity: &str, message: &str, line: u32) -> DocDiagnostic {
        DocDiagnostic {
            severity: severity.to_string(),
            code: Some("E0282".to_string()),
            message: message.to_string(),
            line,
            col: 3,
            source: None,
            note: None,
        }
    }

    fn report_of(items: Vec<DocDiagnostic>) -> DiagnosticsReport {
        DiagnosticsReport {
            file: "src/messages.rs".to_string(),
            errors: items.iter().filter(|d| d.severity == "error").count(),
            warnings: items.iter().filter(|d| d.severity == "warning").count(),
            items,
            preexisting: vec![],
        }
    }

    #[test]
    fn an_error_the_file_already_had_is_not_the_edits() {
        // Two derives the analyzer cannot type on disk; the edit moves them down a line and
        // adds a third error of the same kind, and one of its own.
        let before_text = "#[derive(Deserialize)]\nstruct A;\n#[derive(Deserialize)]\nstruct B;\n";
        let before = report_of(vec![
            diagnostic("error", "type annotations needed", 1),
            diagnostic("error", "type annotations needed", 3),
        ]);
        let text = "use x;\n#[derive(Deserialize)]\nstruct A;\n#[derive(Deserialize)]\nstruct B;\n\
                    #[derive(Deserialize)]\nstruct C(u8);\nfn f() -> u8 { \"\" }\n";
        let mut report = report_of(vec![
            diagnostic("error", "type annotations needed", 2),
            diagnostic("error", "type annotations needed", 4),
            diagnostic("error", "type annotations needed", 6),
            diagnostic("error", "expected u8, found &str", 8),
            diagnostic("warning", "type annotations needed", 2),
        ]);
        set_aside_preexisting(&mut report, text, &before, before_text);
        assert_eq!(report.preexisting.len(), 2, "{:?}", report.preexisting);
        let lines: Vec<u32> = report.items.iter().map(|d| d.line).collect();
        assert_eq!(
            lines,
            vec![6, 8, 2],
            "a third copy and a new severity are the edit's"
        );
        assert_eq!((report.errors, report.warnings), (2, 1));
        let shown = report.render();
        assert!(
            shown.contains("src/messages.rs: 2 error(s), 1 warning(s)"),
            "{shown}"
        );
        assert!(
            shown.contains("2 diagnostic(s) the file already had before this edit are not counted: 2× type annotations needed [E0282]"),
            "{shown}"
        );
    }

    #[test]
    fn errors_on_lines_using_a_removed_symbol_get_a_note() {
        let mut reports = vec![DiagnosticsReport {
            file: "crates/gateway/src/main.rs".to_string(),
            errors: 1,
            warnings: 0,
            items: vec![
                DocDiagnostic {
                    severity: "error".to_string(),
                    code: Some("E0282".to_string()),
                    message: "type annotations needed".to_string(),
                    line: 2,
                    col: 16,
                    source: None,
                    note: None,
                },
                DocDiagnostic {
                    severity: "hint".to_string(),
                    code: None,
                    message: "unused".to_string(),
                    line: 3,
                    col: 1,
                    source: None,
                    note: None,
                },
            ],
            preexisting: vec![],
        }];
        let mut sources = HashMap::new();
        sources.insert(
            "crates/gateway/src/main.rs".to_string(),
            "fn run() {\n    if let Some(s) = workspace::shared_target_dir(&ws) {}\n    let unused = 1;\n}\n".to_string(),
        );
        let missing = vec![(
            "shared_target_dir".to_string(),
            "crates/gateway/src/workspace.rs".to_string(),
        )];
        annotate_missing_symbols(&mut reports, &sources, &missing);
        let note = reports[0].items[0].note.as_deref().unwrap();
        assert!(
            note.contains("`shared_target_dir`")
                && note.contains("crates/gateway/src/workspace.rs"),
            "{note}"
        );
        assert!(reports[0].items[1].note.is_none(), "hints are left alone");
        assert_eq!(
            reports[0].items.len(),
            2,
            "a line that already has an error gets no extra warning"
        );
        assert!(
            reports[0]
                .render()
                .contains("note: this line uses `shared_target_dir`")
        );
    }

    #[test]
    fn silent_lines_using_a_removed_symbol_get_a_synthesised_warning() {
        let mut reports = vec![
            DiagnosticsReport {
                file: "crates/gateway/src/main.rs".to_string(),
                errors: 0,
                warnings: 0,
                items: vec![],
                preexisting: vec![],
            },
            DiagnosticsReport {
                file: "crates/gateway/src/workspace.rs".to_string(),
                errors: 0,
                warnings: 0,
                items: vec![],
                preexisting: vec![],
            },
        ];
        let mut sources = HashMap::new();
        sources.insert(
            "crates/gateway/src/main.rs".to_string(),
            "fn run() {\n    workspace::touch_last_used(&server_workspace);\n}\n".to_string(),
        );
        sources.insert(
            "crates/gateway/src/workspace.rs".to_string(),
            "/// touch_last_used used to live here\npub fn record_last_used() {}\n".to_string(),
        );
        let missing = vec![(
            "touch_last_used".to_string(),
            "crates/gateway/src/workspace.rs".to_string(),
        )];
        annotate_missing_symbols(&mut reports, &sources, &missing);
        assert_eq!(reports[0].warnings, 1);
        assert_eq!(reports[0].items.len(), 1);
        let item = &reports[0].items[0];
        assert_eq!((item.line, item.col), (2, 16));
        assert_eq!(item.code.as_deref(), Some(STALE_REFERENCE));
        assert!(
            item.note
                .as_deref()
                .unwrap()
                .contains("crates/gateway/src/workspace.rs")
        );
        assert!(
            reports[1].items.is_empty(),
            "the file the symbol vanished from is not flagged"
        );
    }
}
