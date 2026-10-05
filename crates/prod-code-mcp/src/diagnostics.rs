//! In-memory diagnostics for a document (roadmap 7.7): what the analyzer thinks of a file,
//! or of a proposed replacement text, without a build and without writing anything.

use crate::session::LspSession;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HallucinationKind {
    InvalidMethodInvocation,
    IncorrectArgumentType,
    BorrowCheckerError,
    SyntaxError,
    UnresolvedIdentifier,
}

impl std::fmt::Display for HallucinationKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidMethodInvocation => write!(f, "InvalidMethodInvocation"),
            Self::IncorrectArgumentType => write!(f, "IncorrectArgumentType"),
            Self::BorrowCheckerError => write!(f, "BorrowCheckerError"),
            Self::SyntaxError => write!(f, "SyntaxError"),
            Self::UnresolvedIdentifier => write!(f, "UnresolvedIdentifier"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HallucinationInterception {
    pub kind: HallucinationKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol_or_target: Option<String>,
    pub message: String,
    pub line: u32,
    pub col: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
}

/// Result of validating a streamed sequence of chunks during agent code generation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamValidationResult {
    pub completed_chunks: usize,
    pub total_chunks: usize,
    pub intercepted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interception: Option<HallucinationInterception>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intercept_chunk_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_report: Option<DiagnosticsReport>,
    pub summary: String,
}

impl StreamValidationResult {
    pub fn render(&self) -> String {
        let mut out = format!("Streamed validation: {}\n", self.summary);
        if let Some(intercept) = &self.interception {
            out.push_str(&format!(
                "  [INTERCEPT at chunk {}]: {}\n",
                self.intercept_chunk_index.map(|i| i + 1).unwrap_or(0),
                intercept.message
            ));
            if let Some(sugg) = &intercept.suggestion {
                out.push_str(&format!("    --> {sugg}\n"));
            }
        }
        if let Some(report) = &self.final_report {
            out.push_str(&report.render());
        }
        out
    }
}

/// Result of incrementally validating an incoming chunk in a stateful streaming session (Phase 7.7).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamChunkResult {
    pub session_id: String,
    pub chunk_index: usize,
    pub accumulated_bytes: usize,
    pub checkpoint_evaluated: bool,
    pub intercepted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interception: Option<HallucinationInterception>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_report: Option<DiagnosticsReport>,
    pub summary: String,
}

impl StreamChunkResult {
    pub fn render(&self) -> String {
        let mut out = format!(
            "Stream chunk {} [{}]: {}\n",
            self.chunk_index, self.session_id, self.summary
        );
        if let Some(intercept) = &self.interception {
            out.push_str(&format!(
                "  [INTERCEPT at chunk {}]: {}\n",
                self.chunk_index, intercept.message
            ));
            if let Some(sugg) = &intercept.suggestion {
                out.push_str(&format!("    --> {sugg}\n"));
            }
        }
        if let Some(report) = &self.final_report {
            out.push_str(&report.render());
        }
        out
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocDiagnostic {
    pub severity: String,
    pub code: Option<String>,
    pub message: String,
    pub line: u32,
    pub col: u32,
    pub source: Option<String>,
    /// Extra explanation added by prod-code (for example that the failing line uses a symbol
    /// the proposed edits removed or renamed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Where the range ends, 1-based line and column, when the analyzer gave one.
    #[serde(skip, default)]
    pub end: Option<(u32, u32)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticsReport {
    pub file: String,
    pub errors: usize,
    pub warnings: usize,
    pub items: Vec<DocDiagnostic>,
    /// Diagnostics the file already had on disk, before the edit under review: the same
    /// severity, code and message on a line with the same text. They are not the edit's, so
    /// they are neither in `items` nor counted in `errors` and `warnings`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preexisting: Vec<DocDiagnostic>,
    /// "type annotations needed" on a `#[derive(...)]` line: the analyzer failing to type its
    /// own expansion of the derive (`serde::Deserialize` does it), which rustc does not report.
    /// Not counted, even in a new file that has no text on disk to compare with (#159).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub in_derive: Vec<DocDiagnostic>,
    /// E0277 that a type is not `Send`, `Sync` or `Unpin`, in a Rust file: rust-analyzer does
    /// not always prove an auto trait rustc proves (through a recursive `async fn`, #327). Shown,
    /// not counted; `cargo check` (`verify: "compile"`) decides.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub auto_trait: Vec<DocDiagnostic>,
    /// Intercepted hallucinations (roadmap 7.7): invalid method invocations, parameter/argument
    /// mismatches, borrow checker errors, and syntax anomalies detected on the fly.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hallucinations: Vec<HallucinationInterception>,
}

impl DiagnosticsReport {
    pub fn is_platform_excluded(&self) -> Option<&str> {
        self.items
            .iter()
            .find(|d| d.code.as_deref() == Some("platform-excluded"))
            .map(|d| d.message.as_str())
    }

    pub fn ok(&self) -> bool {
        self.errors == 0
    }

    pub fn render(&self) -> String {
        if let Some(reason) = self.is_platform_excluded() {
            return format!("{}: platform-excluded ({})\n", self.file, reason);
        }
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
        if !self.in_derive.is_empty() {
            out.push_str(&format!(
                "  ({} \"type annotations needed\" on a #[derive(...)] line are not counted: the \
                 analyzer's own expansion of the derive, which rustc does not report)\n",
                self.in_derive.len()
            ));
        }
        if !self.auto_trait.is_empty() {
            out.push_str(&format!(
                "  ({} unproven Send/Sync/Unpin bound(s) are not counted: rust-analyzer does not \
                 always prove what rustc does; `cargo check` or `verify: \"compile\"` decides)\n",
                self.auto_trait.len()
            ));
            for d in &self.auto_trait {
                out.push_str(&format!(
                    "    unconfirmed: {} ({}:{}:{})\n",
                    d.message.lines().next().unwrap_or(""),
                    self.file,
                    d.line,
                    d.col
                ));
            }
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
        if !self.hallucinations.is_empty() {
            out.push_str("  === Intercepted Hallucinations (Phase 7.7) ===\n");
            for h in &self.hallucinations {
                out.push_str(&format!(
                    "  [INTERCEPT] {}: {} ({}:{}:{})\n",
                    h.kind,
                    h.message.lines().next().unwrap_or(""),
                    self.file,
                    h.line,
                    h.col
                ));
                if let Some(sugg) = &h.suggestion {
                    out.push_str(&format!("    --> {sugg}\n"));
                }
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

fn refresh_hallucinations(report: &mut DiagnosticsReport) {
    report.hallucinations = report
        .items
        .iter()
        .filter(|diagnostic| diagnostic.severity == "error")
        .filter_map(classify_hallucination)
        .collect();
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
        // A file the analyzer panicked on, or that no crate includes, was not checked; that it
        // was not checked before the edit either does not make it checked now (#94, #467).
        if d.code.as_deref() == Some(prod_code_protocol::ANALYZER_PANIC_CODE)
            || d.code.as_deref() == Some(UNLINKED_FILE)
            || d.code.as_deref() == Some(INVALID_DIAGNOSTICS)
        {
            report.items.push(d);
            continue;
        }
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
    refresh_hallucinations(report);
}

/// Moves to `report.in_derive` every E0282 on a line of `text` that is a `#[derive(...)]`
/// attribute: an inference failure inside the analyzer's expansion of the derive, not in code
/// anyone wrote. Moves to `report.auto_trait` every E0277 of a Rust file that a type is not
/// `Send`, `Sync` or `Unpin` (#327).
fn set_aside_derive_expansions(report: &mut DiagnosticsReport, text: &str) {
    let rust = report.file.ends_with(".rs");
    let items = std::mem::take(&mut report.items);
    for d in items {
        let line = text
            .lines()
            .nth(d.line.saturating_sub(1) as usize)
            .unwrap_or("");
        if d.code.as_deref() == Some("E0282") && line.trim_start().starts_with("#[derive(") {
            report.in_derive.push(d);
        } else if rust && d.code.as_deref() == Some("E0277") && is_auto_trait_bound(&d.message) {
            report.auto_trait.push(d);
        } else {
            report.items.push(d);
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
    refresh_hallucinations(report);
}

/// Whether an E0277 message is about an auto trait: "the trait bound `NonNull<()>: Send` is
/// not satisfied", "`Rc<u8>` cannot be sent between threads safely", "... cannot be shared
/// between threads safely".
fn is_auto_trait_bound(message: &str) -> bool {
    let first = message.lines().next().unwrap_or("");
    [": Send`", ": Sync`", ": Unpin`"]
        .iter()
        .any(|bound| first.contains(bound))
        || first.contains("cannot be sent between threads safely")
        || first.contains("cannot be shared between threads safely")
}

/// rust-analyzer's code for a file that no crate includes: it offers no semantic service there,
/// so it reports no type error, no unresolved name, nothing but this hint (#467).
pub const UNLINKED_FILE: &str = "unlinked-file";

/// What a validation says of an `unlinked-file`: why it counts, that it is not a type error, and
/// what would check the file.
const UNLINKED_NOTE: &str = "rust-analyzer includes this file in no crate, so it was not \
     type-checked and no name in it was resolved; it is counted as an error because an unchecked file is not a clean \
     one, not because an error was found in it. A new module is checked together with the file \
     that declares it (`code_validate_edits`, `prod-code validate --with`). A new Cargo target \
     (tests/, examples/, benches/, src/bin/) reaches the analyzer only once it exists on disk and \
     the workspace reloads; `compile: true` (`--compile`) runs `cargo check --workspace \
     --all-targets`, which compiles the file only when a target includes it (Cargo finds tests/*.rs \
     itself), and this item still counts";

/// A proposal the analyzer did not check is not one it found clean (#467): rust-analyzer answers
/// a file no crate includes with one `unlinked-file` hint, and 0 errors would validate whatever
/// the file says. In a validation that hint is an error with a note; the message stays the
/// analyzer's. Read-only diagnostics of a file on disk leave it a hint.
fn refuse_unchecked(report: &mut DiagnosticsReport) {
    let mut refused = false;
    for d in report.items.iter_mut() {
        if d.code.as_deref() == Some(UNLINKED_FILE) {
            d.severity = "error".to_string();
            d.note = Some(UNLINKED_NOTE.to_string());
            refused = true;
        }
    }
    if refused {
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
}

/// LSP `SymbolKind::Method`: 6
const SYMBOL_KIND_METHOD: u64 = 6;
/// LSP `SymbolKind::Constructor`: 9
const SYMBOL_KIND_CONSTRUCTOR: u64 = 9;
/// LSP `SymbolKind::Function`: 12
const SYMBOL_KIND_FUNCTION: u64 = 12;
/// LSP `SymbolKind::Variable`: rust-analyzer lists a function's `let` bindings under it.
const SYMBOL_KIND_VARIABLE: u64 = 13;

/// Every symbol name in a `textDocument/documentSymbol` result (flat or hierarchical) that
/// another file could refer to. A local variable is listed too, and it is not one of them: a
/// `let edit` removed from one function is not what another file's `let edit` names (#136).
/// Declarations and returned properties nested inside a function, method, or constructor
/// are local execution details that external files cannot refer to (#818).
fn symbol_names(result: &serde_json::Value) -> BTreeSet<String> {
    fn walk(value: &serde_json::Value, out: &mut BTreeSet<String>) {
        match value {
            serde_json::Value::Array(items) => items.iter().for_each(|i| walk(i, out)),
            serde_json::Value::Object(map) => {
                let kind = map.get("kind").and_then(|k| k.as_u64());
                let is_local = kind == Some(SYMBOL_KIND_VARIABLE);
                let is_scoped = matches!(
                    kind,
                    Some(SYMBOL_KIND_FUNCTION | SYMBOL_KIND_METHOD | SYMBOL_KIND_CONSTRUCTOR)
                );
                if let Some(name) = map.get("name").and_then(|n| n.as_str())
                    && !is_local
                {
                    out.insert(name.to_string());
                }
                if !is_scoped && let Some(children) = map.get("children") {
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

/// A Rust identifier token found in executable source code (outside comments, strings,
/// character literals, numbers and lifetimes).
#[derive(Debug, Clone, PartialEq, Eq)]
struct RustIdent {
    name: String,
    line: u32,
    col: u32,
}

/// Converts a 0-based byte offset in `text` to 1-based (line, UTF-16 column).
fn byte_to_line_col(line_starts: &[usize], text: &str, byte_offset: usize) -> (u32, u32) {
    let line_idx = line_starts
        .partition_point(|&s| s <= byte_offset)
        .saturating_sub(1);
    let line_no = line_idx as u32 + 1;
    let line_start = line_starts[line_idx];
    let col = text[line_start..byte_offset]
        .chars()
        .map(|c| c.len_utf16())
        .sum::<usize>() as u32
        + 1;
    (line_no, col)
}

/// Checks whether a raw string (e.g. `r"..."`, `r#"..."#`, `br#"..."#`, `cr#"..."#`) starts at `i`.
/// Returns `Some((content_start_char_idx, num_hashes))` if so.
fn raw_string_start(chars: &[(usize, char)], i: usize) -> Option<(usize, usize)> {
    let at = |idx: usize| chars.get(idx).map(|&(_, c)| c);
    let mut p = i;
    if matches!(at(p), Some('b') | Some('c')) && at(p + 1) == Some('r') {
        p += 2;
    } else if at(p) == Some('r') {
        p += 1;
    } else {
        return None;
    }
    let mut hashes = 0;
    while at(p) == Some('#') {
        hashes += 1;
        p += 1;
    }
    if at(p) == Some('"') {
        Some((p + 1, hashes))
    } else {
        None
    }
}

/// Checks whether a quoted string (`"..."`, `b"..."`, `c"..."`) starts at `i`.
/// Returns `Some(content_start_char_idx)` if so.
fn quoted_string_start(chars: &[(usize, char)], i: usize) -> Option<usize> {
    let at = |idx: usize| chars.get(idx).map(|&(_, c)| c);
    if at(i) == Some('"') {
        Some(i + 1)
    } else if matches!(at(i), Some('b') | Some('c')) && at(i + 1) == Some('"') {
        Some(i + 2)
    } else {
        None
    }
}

/// Scans `text` for Rust identifier tokens, skipping whitespace, line/doc/nested block
/// comments, string literals (ordinary, raw, byte, C), character literals, numbers,
/// and lifetime identifiers. Returns identifier tokens with their 1-based line and 1-based
/// UTF-16 column.
fn rust_code_identifiers(text: &str) -> Vec<RustIdent> {
    let line_starts: Vec<usize> = std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .collect();

    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let at = |idx: usize| chars.get(idx).map(|&(_, c)| c);
    let offset = |idx: usize| chars.get(idx).map_or(text.len(), |&(o, _)| o);
    let ident_start = |c: char| c == '_' || unicode_ident::is_xid_start(c);
    let ident_char = |c: char| c == '_' || unicode_ident::is_xid_continue(c);

    let mut tokens = Vec::new();
    let mut i = 0;

    while let Some(c) = at(i) {
        if c.is_whitespace() {
            i += 1;
        } else if c == '/' && at(i + 1) == Some('/') {
            // Line comment (including /// doc comments and //! inner doc comments)
            i += 2;
            while at(i).is_some_and(|ch| ch != '\n') {
                i += 1;
            }
        } else if c == '/' && at(i + 1) == Some('*') {
            // Block comment (including /** doc comments and nested /* /* */ */)
            let mut depth = 1usize;
            i += 2;
            while i < chars.len() && depth > 0 {
                if at(i) == Some('/') && at(i + 1) == Some('*') {
                    depth += 1;
                    i += 2;
                } else if at(i) == Some('*') && at(i + 1) == Some('/') {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
        } else if let Some((content_start, hashes)) = raw_string_start(&chars, i) {
            // Raw string (r"...", r#"..."#, br"...", br#"..."#, cr"...", cr#"..."#)
            let mut p = content_start;
            while p < chars.len() {
                if at(p) == Some('"') && (1..=hashes).all(|k| at(p + k) == Some('#')) {
                    p += 1 + hashes;
                    break;
                }
                p += 1;
            }
            i = p;
        } else if let Some(content_start) = quoted_string_start(&chars, i) {
            // Quoted string ("...", b"...", c"...")
            let mut p = content_start;
            while p < chars.len() {
                match at(p) {
                    Some('\\') => p += 2,
                    Some('"') => {
                        p += 1;
                        break;
                    }
                    _ => p += 1,
                }
            }
            i = p;
        } else if c == 'b' && at(i + 1) == Some('\'') {
            // Byte character literal: b'a', b'\'', b'\\'
            let mut j = i + 2;
            if at(j) == Some('\\') {
                j += 1;
                while at(j).is_some_and(|ch| ch != '\'' && ch != '\n') {
                    j += 1;
                }
                if at(j) == Some('\'') {
                    j += 1;
                }
            } else if at(j).is_some() && at(j + 1) == Some('\'') && at(j) != Some('\n') {
                j += 2;
            } else {
                while at(j).is_some_and(|ch| ch != '\'' && ch != '\n') {
                    j += 1;
                }
                if at(j) == Some('\'') {
                    j += 1;
                }
            }
            i = j;
        } else if c == '\'' {
            // Character literal vs lifetime
            if at(i + 1) == Some('\\') {
                // Escaped char literal: '\'', '\\', '\n', '\u{1F600}'
                let mut j = i + 2;
                while at(j).is_some_and(|ch| ch != '\'' && ch != '\n') {
                    j += 1;
                }
                if at(j) == Some('\'') {
                    j += 1;
                }
                i = j;
            } else if at(i + 1).is_some() && at(i + 2) == Some('\'') && at(i + 1) != Some('\n') {
                // Single character literal: 'a', '0', ' '
                i += 3;
            } else if at(i + 1).is_some_and(ident_start) {
                // Lifetime identifier: 'static, 'a, 'r#life
                i += 1;
                if at(i) == Some('r')
                    && at(i + 1) == Some('#')
                    && at(i + 2).is_some_and(ident_start)
                {
                    i += 2;
                }
                while at(i).is_some_and(ident_char) {
                    i += 1;
                }
            } else {
                i += 1;
            }
        } else if c == 'r' && at(i + 1) == Some('#') && at(i + 2).is_some_and(ident_start) {
            // Raw identifier: r#foo
            let token_start = offset(i);
            i += 2;
            let name_start = offset(i);
            while at(i).is_some_and(ident_char) {
                i += 1;
            }
            let name_end = offset(i);
            let name = &text[name_start..name_end];
            let (line, col) = byte_to_line_col(&line_starts, text, token_start);
            tokens.push(RustIdent {
                name: name.to_string(),
                line,
                col,
            });
        } else if ident_start(c) {
            // Normal identifier: foo
            let token_start = offset(i);
            while at(i).is_some_and(ident_char) {
                i += 1;
            }
            let token_end = offset(i);
            let name = &text[token_start..token_end];
            let (line, col) = byte_to_line_col(&line_starts, text, token_start);
            tokens.push(RustIdent {
                name: name.to_string(),
                line,
                col,
            });
        } else if c.is_ascii_digit() {
            // Number literal: 123, 0x1f, 1.5e3
            while at(i).is_some_and(|ch| ch == '_' || ch.is_ascii_alphanumeric())
                || (at(i) == Some('.') && at(i + 1).is_some_and(|ch| ch.is_ascii_digit()))
            {
                i += 1;
            }
        } else {
            i += 1;
        }
    }

    tokens
}

/// Finds 1-based UTF-16 column numbers where `name` appears in `line` as a whole identifier.
fn identifier_columns(line: &str, name: &str) -> Vec<u32> {
    if name.is_empty() {
        return Vec::new();
    }
    let bytes = line.as_bytes();
    let mut cols = Vec::new();
    let mut from = 0;
    while let Some(pos) = line[from..].find(name) {
        let start = from + pos;
        let end = start + name.len();
        let before_ok =
            start == 0 || !(bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_');
        let after_ok =
            end >= bytes.len() || !(bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_');
        if before_ok && after_ok {
            let col = line[..start]
                .chars()
                .map(|c| c.len_utf16())
                .sum::<usize>() as u32
                + 1;
            cols.push(col);
        }
        from = end;
    }
    cols
}

/// Whether `line` mentions `name` as a whole identifier.
fn mentions_identifier(line: &str, name: &str) -> bool {
    !identifier_columns(line, name).is_empty()
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

const INVALID_DIAGNOSTICS: &str = "prod-code-invalid-diagnostics";

/// A missing or malformed required report is unavailable evidence, not a clean file. Keep
/// this as a diagnostic so every CLI/MCP consumer preserves its unsuccessful status.
fn invalid_report(file: &str, reason: &str) -> DiagnosticsReport {
    DiagnosticsReport {
        file: file.to_string(),
        errors: 1,
        warnings: 0,
        items: vec![DocDiagnostic {
            severity: "error".to_string(),
            code: Some(INVALID_DIAGNOSTICS.to_string()),
            message: format!(
                "the analyzer returned invalid diagnostics: {reason}; the file was not validated. Retry the language server or use an explicit compiler check"
            ),
            line: 1,
            col: 1,
            source: Some("prod-code".to_string()),
            note: None,
            end: None,
        }],
        preexisting: Vec::new(),
        in_derive: Vec::new(),
        auto_trait: Vec::new(),
        hallucinations: Vec::new(),
    }
}

fn diagnostic_position(value: Option<&serde_json::Value>) -> Result<(u32, u32), String> {
    let value = value.ok_or("missing range endpoint")?;
    let coordinate = |key| {
        value
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .and_then(|n| n.checked_add(1))
            .ok_or_else(|| format!("invalid or unrepresentable {key} coordinate"))
    };
    Ok((coordinate("line")?, coordinate("character")?))
}

fn parse_diagnostic(d: &serde_json::Value) -> Result<DocDiagnostic, String> {
    let range = d.get("range").ok_or("missing diagnostic range")?;
    let start = diagnostic_position(range.get("start"))?;
    let end = diagnostic_position(range.get("end"))?;
    if end < start {
        return Err("diagnostic range ends before it starts".into());
    }
    let severity = match d.get("severity") {
        None => "error",
        Some(v) => match v.as_u64() {
            Some(1) => "error",
            Some(2) => "warning",
            Some(3) => "info",
            Some(4) => "hint",
            _ => return Err("invalid diagnostic severity".into()),
        },
    };
    let code = match d.get("code") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(n) if n.as_i64().is_some() => Some(n.to_string()),
        Some(_) => return Err("diagnostic code is neither a string nor an integer".into()),
    };
    let source = match d.get("source") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(_) => return Err("diagnostic source is not a string".into()),
    };
    let message = d
        .get("message")
        .and_then(serde_json::Value::as_str)
        .ok_or("missing diagnostic message")?
        .to_string();
    Ok(DocDiagnostic {
        severity: severity.into(),
        code,
        message,
        line: start.0,
        col: start.1,
        source,
        note: None,
        end: Some(end),
    })
}

pub fn is_borrow_checker_error_code(code: &str) -> bool {
    let clean = code.trim().trim_start_matches('[').trim_end_matches(']');
    matches!(
        clean,
        "E0382"
            | "E0499"
            | "E0502"
            | "E0503"
            | "E0505"
            | "E0506"
            | "E0507"
            | "E0515"
            | "E0521"
            | "E0596"
            | "E0597"
            | "E0716"
    )
}

fn extract_method_name(msg: &str) -> Option<String> {
    if let Some(start) = msg.find("no method named `") {
        let rest = &msg[start + "no method named `".len()..];
        if let Some(end) = rest.find('`') {
            return Some(rest[..end].to_string());
        }
    }
    if let Some(start) = msg.find("Property '") {
        let rest = &msg[start + "Property '".len()..];
        if let Some(end) = rest.find('\'') {
            return Some(rest[..end].to_string());
        }
    }
    if let Some(idx) = msg.find(" undefined") {
        let prefix = &msg[..idx];
        if let Some(dot) = prefix.rfind('.') {
            return Some(prefix[dot + 1..].trim().to_string());
        }
    }
    if let Some(start) = msg.find("Cannot access member '") {
        let rest = &msg[start + "Cannot access member '".len()..];
        if let Some(end) = rest.find('\'') {
            return Some(rest[..end].to_string());
        }
    }
    if let Some(start) = msg.find("no member named '") {
        let rest = &msg[start + "no member named '".len()..];
        if let Some(end) = rest.find('\'') {
            return Some(rest[..end].to_string());
        }
    }
    if let Some(start) = msg.find("has no attribute '") {
        let rest = &msg[start + "has no attribute '".len()..];
        if let Some(end) = rest.find('\'') {
            return Some(rest[..end].to_string());
        }
    }
    None
}

pub fn classify_hallucination(d: &DocDiagnostic) -> Option<HallucinationInterception> {
    let msg_lower = d.message.to_ascii_lowercase();
    let code_str = d.code.as_deref().unwrap_or("");

    if is_borrow_checker_error_code(code_str)
        || msg_lower.contains("borrow")
        || msg_lower.contains("use of moved value")
        || msg_lower.contains("does not live long enough")
        || msg_lower.contains("returns a value referencing data owned by the current function")
    {
        return Some(HallucinationInterception {
            kind: HallucinationKind::BorrowCheckerError,
            symbol_or_target: None,
            message: d.message.clone(),
            line: d.line,
            col: d.col,
            suggestion: Some("Review value ownership, adjust borrow scopes, clone, or adjust reference lifetimes".to_string()),
        });
    }

    if code_str == "unresolved-method"
        || code_str == "no-such-field"
        || code_str == "2339"
        || code_str == "2551"
        || msg_lower.contains("no method named")
        || msg_lower.contains("does not exist on type")
        || (msg_lower.contains("has no field or method") && msg_lower.contains("undefined"))
        || msg_lower.contains("cannot access member")
        || msg_lower.contains("no member named")
        || msg_lower.contains("has no attribute")
    {
        let target = extract_method_name(&d.message);
        return Some(HallucinationInterception {
            kind: HallucinationKind::InvalidMethodInvocation,
            symbol_or_target: target,
            message: d.message.clone(),
            line: d.line,
            col: d.col,
            suggestion: Some("Verify method existence on type or check trait imports".to_string()),
        });
    }

    if code_str == "syntax-error"
        || code_str == "parse-error"
        || msg_lower.contains("syntax error")
        || (msg_lower.contains("expected ")
            && (msg_lower.contains("found `;`")
                || msg_lower.contains("found `}`")
                || msg_lower.contains("found `{`")
                || msg_lower.contains("expected token")))
        || msg_lower.contains("unclosed delimiter")
        || msg_lower.contains("unexpected token")
    {
        return Some(HallucinationInterception {
            kind: HallucinationKind::SyntaxError,
            symbol_or_target: None,
            message: d.message.clone(),
            line: d.line,
            col: d.col,
            suggestion: Some("Correct unbalanced syntax or missing delimiter".to_string()),
        });
    }

    if code_str == "E0308"
        || code_str == "E0061"
        || code_str == "type-mismatch"
        || code_str == "2345"
        || code_str == "2554"
        || msg_lower.contains("mismatched types")
        || (msg_lower.contains("expected ") && msg_lower.contains("found "))
        || msg_lower.contains("is not assignable to parameter")
        || (msg_lower.contains("this function takes") && msg_lower.contains("arguments but"))
        || (msg_lower.contains("cannot use") && msg_lower.contains("value in argument"))
        || msg_lower.contains("cannot be assigned to parameter")
    {
        return Some(HallucinationInterception {
            kind: HallucinationKind::IncorrectArgumentType,
            symbol_or_target: None,
            message: d.message.clone(),
            line: d.line,
            col: d.col,
            suggestion: Some("Verify function signature and argument types".to_string()),
        });
    }

    if code_str == "E0425"
        || code_str == "unresolved-ident"
        || code_str == "unresolved-path"
        || code_str == "2304"
        || msg_lower.contains("cannot find value")
        || msg_lower.contains("cannot find function")
        || msg_lower.contains("cannot find name")
        || msg_lower.contains("undefined:")
    {
        return Some(HallucinationInterception {
            kind: HallucinationKind::UnresolvedIdentifier,
            symbol_or_target: None,
            message: d.message.clone(),
            line: d.line,
            col: d.col,
            suggestion: Some("Check symbol spelling or add missing import".to_string()),
        });
    }

    None
}

fn parse_items(file: &str, result: &serde_json::Value) -> DiagnosticsReport {
    // This client never sends a previousResultId, so an unchanged report has no cached
    // evidence to refer to. Older adapters omit kind but still provide the complete items.
    if result
        .get("kind")
        .is_some_and(|kind| kind.as_str() != Some("full"))
    {
        return invalid_report(
            file,
            "expected a full report; no previous result was supplied",
        );
    }
    let Some(raw_items) = result
        .get("items")
        .and_then(serde_json::Value::as_array)
        .or_else(|| result.as_array())
    else {
        return invalid_report(file, "required items array is missing or malformed");
    };
    let mut items = Vec::with_capacity(raw_items.len());
    for (index, raw) in raw_items.iter().enumerate() {
        match parse_diagnostic(raw) {
            Ok(d) if d.code.as_deref() == Some("inactive-code") => {}
            Ok(d) => items.push(d),
            Err(reason) => return invalid_report(file, &format!("diagnostic {index}: {reason}")),
        }
    }
    let hallucinations = items
        .iter()
        .filter(|d| d.severity == "error")
        .filter_map(classify_hallucination)
        .collect();
    DiagnosticsReport {
        file: file.to_string(),
        errors: items.iter().filter(|d| d.severity == "error").count(),
        warnings: items.iter().filter(|d| d.severity == "warning").count(),
        items,
        preexisting: Vec::new(),
        in_derive: Vec::new(),
        auto_trait: Vec::new(),
        hallucinations,
    }
}

/// These diagnostics come from the supported source-language engines, not a manifest or
/// document validator. Refuse the entire batch before querying any part of it (#465).
fn ensure_source_file(file: &Path) -> Result<()> {
    if is_parser_validated_file(file) {
        return Ok(());
    }
    let supported = matches!(
        crate::lang::language_id_for_path(file),
        "rust"
            | "go"
            | "python"
            | "typescript"
            | "typescriptreact"
            | "javascript"
            | "javascriptreact"
            | "c"
            | "cpp"
            | "objective-c"
            | "objective-cpp"
            | "swift"
            | "java"
            | "kotlin"
            | "csharp"
            | "scala"
            | "zig"
            | "nim"
            | "d"
            | "php"
            | "ruby"
            | "dart"
            | "lua"
            | "elixir"
    ) || crate::lang::is_header(file);
    anyhow::ensure!(
        supported,
        "semantic diagnostics are not supported for {}; no validation was performed. \
         For manifests and lockfiles, use prod-code shadow-run with the \
         appropriate parser or build command on the complete proposal (for Rust, cargo check \
         --workspace --all-targets)",
        file.display()
    );
    Ok(())
}

/// Whether `file` is a JSON manifest or document whose syntax is validated directly (#733).
pub fn is_json_file(file: &Path) -> bool {
    crate::lang::language_id_for_path(file) == "json"
}

/// Whether `file` is validated directly by an internal parser without an LSP session (#733, #778).
pub fn is_parser_validated_file(file: &Path) -> bool {
    matches!(
        crate::lang::language_id_for_path(file),
        "json" | "markdown" | "xml"
    )
}

pub use crate::markdown::validate_markdown;
pub use crate::xml_svg::validate_xml;

/// SVG syntax validator (#778).
pub fn validate_svg(shown: &str, text: &str) -> DiagnosticsReport {
    validate_xml(shown, text, true)
}

/// Validates file content using built-in syntax parsers (#733, #778).
pub fn validate_file_content(shown: &str, file: &Path, text: &str) -> DiagnosticsReport {
    match crate::lang::language_id_for_path(file) {
        "json" => validate_json(shown, text),
        "markdown" => validate_markdown(shown, text),
        "xml" => {
            let is_svg = file.extension().and_then(|e| e.to_str()) == Some("svg");
            validate_xml(shown, text, is_svg)
        }
        _ => unreachable!("validate_file_content called on non-parser-validated file"),
    }
}

/// JSON syntax validator for manifests and JSON configuration files (#733).
pub fn validate_json(shown: &str, text: &str) -> DiagnosticsReport {
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(_) => DiagnosticsReport {
            file: shown.to_string(),
            errors: 0,
            warnings: 0,
            items: Vec::new(),
            preexisting: Vec::new(),
            in_derive: Vec::new(),
            auto_trait: Vec::new(),
            hallucinations: Vec::new(),
        },
        Err(err) => {
            let line = (err.line() as u32).max(1);
            let col = (err.column() as u32).max(1);
            let diag = DocDiagnostic {
                severity: "error".to_string(),
                message: format!("JSON syntax error: {err}"),
                code: Some("json-syntax".to_string()),
                line,
                col,
                source: None,
                end: None,
                note: None,
            };
            let hallucinations = vec![HallucinationInterception {
                kind: HallucinationKind::SyntaxError,
                symbol_or_target: None,
                message: diag.message.clone(),
                line,
                col,
                suggestion: Some("Correct invalid JSON syntax".to_string()),
            }];
            DiagnosticsReport {
                file: shown.to_string(),
                errors: 1,
                warnings: 0,
                items: vec![diag],
                preexisting: Vec::new(),
                in_derive: Vec::new(),
                auto_trait: Vec::new(),
                hallucinations,
            }
        }
    }
}

/// Diagnostics of `file` as it is on disk.
pub async fn diagnostics(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
) -> Result<DiagnosticsReport> {
    if is_parser_validated_file(file) {
        let abs = if file.is_absolute() {
            file.to_path_buf()
        } else {
            root.join(file)
        };
        let text = std::fs::read_to_string(&abs)
            .with_context(|| format!("cannot read {}", file.display()))?;
        return Ok(validate_file_content(&display(root, file), file, &text));
    }
    ensure_source_file(file)?;
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
    let mut report = parse_items(&display(root, file), &result);
    let abs = if file.is_absolute() {
        file.to_path_buf()
    } else {
        root.join(file)
    };
    // What validation sets aside, a file on disk sets aside too (#327).
    if let Ok(text) = std::fs::read_to_string(&abs) {
        set_aside_derive_expansions(&mut report, &text);
    }
    Ok(report)
}

/// Diagnostics of `file` as if its content were `new_text`; nothing is written.
pub async fn validate_text(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    new_text: &str,
) -> Result<DiagnosticsReport> {
    if is_parser_validated_file(file) {
        return Ok(validate_file_content(&display(root, file), file, new_text));
    }
    ensure_source_file(file)?;
    let shown = display(root, file);
    // The file as it is on disk, read on the validation engine: it has no overlay for this
    // session, and it is the engine the gateway warms. The main engine is cold for the file's
    // diagnostics after a restart, and asking it cost 21 s of a 24 s validation (#235).
    let abs = if file.is_absolute() {
        file.to_path_buf()
    } else {
        root.join(file)
    };
    let before = if abs.is_file() {
        let mut checkout = LspSession::open_for_validation(remote, root, Some(file)).await?;
        let before = on_disk(&mut checkout, root, file, &shown).await;
        checkout.close().await;
        before
    } else {
        None
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
    set_aside_derive_expansions(&mut report, new_text);
    refuse_unchecked(&mut report);
    if file.extension().and_then(|e| e.to_str()) == Some("swift") && report.errors > 0 {
        let mut reports = vec![report];
        let edits = [(file.to_path_buf(), new_text.to_string())];
        let _ = reconcile_swift_cross_target_diagnostics(remote, root, &edits, &mut reports).await;
        report = reports.pop().unwrap();
    }
    Ok(report)
}

/// Maximum size of a single stream chunk (1 MB).
pub const MAX_STREAM_CHUNK_BYTES: usize = 1024 * 1024;
/// Maximum accumulated source buffer allowed per stream session (16 MB).
pub const MAX_STREAM_SESSION_BYTES: usize = 16 * 1024 * 1024;
/// Maximum number of concurrently retained active stream sessions.
pub const MAX_ACTIVE_STREAM_SESSIONS: usize = 128;
/// Maximum number of tombstone entries tracked to prevent silent prefix loss.
pub const MAX_STREAM_TOMBSTONES: usize = 1024;

/// Unique key scoping a stream session to its gateway remote, canonical workspace root, canonical file, and session ID.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StreamSessionKey {
    pub remote: SocketAddr,
    pub root: PathBuf,
    pub file: PathBuf,
    pub session_id: String,
}

impl StreamSessionKey {
    pub fn new(remote: SocketAddr, root: &Path, file: &Path, session_id: &str) -> Self {
        let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        let abs_file = if file.is_absolute() {
            file.to_path_buf()
        } else {
            canonical_root.join(file)
        };
        let canonical_file = std::fs::canonicalize(&abs_file).unwrap_or(abs_file);
        Self {
            remote,
            root: canonical_root,
            file: canonical_file,
            session_id: session_id.to_string(),
        }
    }
}

/// Stateful stream validation session (Roadmap 7.7).
#[derive(Debug, Clone)]
pub struct StreamSession {
    pub key: StreamSessionKey,
    pub accumulated: String,
    pub chunk_count: usize,
    pub last_activity: Instant,
}

/// Global manager for stateful streaming validation sessions (Roadmap 7.7).
/// Allows external producers (CLI line-by-line stdin pipe, MCP stream calls) to feed incremental chunks,
/// evaluate syntax and type checks at syntactic checkpoints, and interrupt generation on-the-fly.
pub struct StreamSessionManager {
    sessions: Mutex<HashMap<StreamSessionKey, StreamSession>>,
    tombstones: Mutex<HashMap<StreamSessionKey, Instant>>,
}

static STREAM_MANAGER: OnceLock<StreamSessionManager> = OnceLock::new();
static BATCH_COUNTER: AtomicU64 = AtomicU64::new(1);

pub fn stream_manager() -> &'static StreamSessionManager {
    STREAM_MANAGER.get_or_init(StreamSessionManager::new)
}

pub fn next_batch_counter() -> u64 {
    BATCH_COUNTER.fetch_add(1, Ordering::Relaxed)
}

fn stream_session_dir() -> PathBuf {
    let dir = if let Some(cache_home) = std::env::var_os("XDG_CACHE_HOME") {
        PathBuf::from(cache_home).join("prod-code/stream-sessions")
    } else if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        if cfg!(target_os = "macos") {
            home.join("Library/Caches/prod-code/stream-sessions")
        } else {
            home.join(".cache/prod-code/stream-sessions")
        }
    } else {
        let user = std::env::var("USER").unwrap_or_else(|_| "default".to_string());
        std::env::temp_dir().join(format!("prod-code-{user}-stream-sessions"))
    };

    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        builder.mode(0o700);
        let _ = builder.create(&dir);
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = std::fs::metadata(&dir) {
            let mut perms = metadata.permissions();
            perms.set_mode(0o700);
            let _ = std::fs::set_permissions(&dir, perms);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = std::fs::create_dir_all(&dir);
    }

    dir
}

fn write_private_session_file(path: &Path, content: &str) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(content.as_bytes())?;
        file.flush()?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, content)?;
    }
    Ok(())
}

fn stream_session_disk_path(key: &StreamSessionKey) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.remote.hash(&mut hasher);
    key.root.hash(&mut hasher);
    key.file.hash(&mut hasher);
    key.session_id.hash(&mut hasher);
    let hash = hasher.finish();
    stream_session_dir().join(format!("{:016x}.session", hash))
}

impl StreamSessionManager {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            tombstones: Mutex::new(HashMap::new()),
        }
    }

    pub fn reset_session(&self, remote: SocketAddr, root: &Path, file: &Path, session_id: &str) {
        let key = StreamSessionKey::new(remote, root, file, session_id);
        if let Ok(mut map) = self.sessions.lock() {
            map.remove(&key);
        }
        if let Ok(mut tombstones) = self.tombstones.lock() {
            tombstones.remove(&key);
        }
        let _ = std::fs::remove_file(stream_session_disk_path(&key));
    }

    fn insert_tombstone(tombstones: &mut HashMap<StreamSessionKey, Instant>, key: StreamSessionKey) {
        if !tombstones.contains_key(&key) && tombstones.len() >= MAX_STREAM_TOMBSTONES {
            if let Some(oldest) = tombstones
                .iter()
                .min_by_key(|(_, t)| *t)
                .map(|(k, _)| k.clone())
            {
                tombstones.remove(&oldest);
            }
        }
        tombstones.insert(key, Instant::now());
    }

    pub fn tombstone_count(&self) -> usize {
        self.tombstones.lock().map(|t| t.len()).unwrap_or(0)
    }

    fn remove_session_key(&self, key: &StreamSessionKey) {
        if let Ok(mut map) = self.sessions.lock() {
            map.remove(key);
        }
        if let Ok(mut tombstones) = self.tombstones.lock() {
            Self::insert_tombstone(&mut tombstones, key.clone());
        }
        let _ = std::fs::remove_file(stream_session_disk_path(key));
    }

    pub fn prune_stale(&self, max_age: std::time::Duration) {
        let now = Instant::now();
        if let Ok(mut map) = self.sessions.lock() {
            let mut expired = Vec::new();
            for (k, s) in map.iter() {
                if now.duration_since(s.last_activity) >= max_age {
                    expired.push(k.clone());
                }
            }
            if !expired.is_empty() {
                for k in &expired {
                    map.remove(k);
                    let _ = std::fs::remove_file(stream_session_disk_path(k));
                }
                if let Ok(mut tombstones) = self.tombstones.lock() {
                    for k in expired {
                        Self::insert_tombstone(&mut tombstones, k);
                    }
                }
            }
        }
        if let Ok(mut tombstones) = self.tombstones.lock() {
            tombstones.retain(|_, t| now.duration_since(*t) < std::time::Duration::from_secs(900));
        }
        let dir = stream_session_dir();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("session") {
                    if let Ok(meta) = std::fs::metadata(&path) {
                        if let Ok(modified) = meta.modified() {
                            if let Ok(age) = std::time::SystemTime::now().duration_since(modified) {
                                if age >= std::cmp::max(max_age, std::time::Duration::from_secs(300)) {
                                    let _ = std::fs::remove_file(&path);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// Feed a chunk into a stateful streaming session and validate incrementally.
    pub async fn feed_chunk(
        &self,
        remote: SocketAddr,
        root: &Path,
        file: &Path,
        session_id: &str,
        chunk: &str,
        is_last: bool,
        reset: bool,
        borrow_check: bool,
    ) -> Result<StreamChunkResult> {
        if chunk.len() > MAX_STREAM_CHUNK_BYTES {
            anyhow::bail!(
                "stream chunk size ({} bytes) exceeds maximum limit of {} bytes",
                chunk.len(),
                MAX_STREAM_CHUNK_BYTES
            );
        }

        self.prune_stale(std::time::Duration::from_secs(300));
        let key = StreamSessionKey::new(remote, root, file, session_id);

        let (accumulated, chunk_index) = {
            let mut map = self.sessions.lock().unwrap();

            if reset {
                map.remove(&key);
                if let Ok(mut tombstones) = self.tombstones.lock() {
                    tombstones.remove(&key);
                }
                let _ = std::fs::remove_file(stream_session_disk_path(&key));
            } else {
                let is_tombstone = self
                    .tombstones
                    .lock()
                    .map(|t| t.contains_key(&key))
                    .unwrap_or(false);
                if is_tombstone {
                    let _ = std::fs::remove_file(stream_session_disk_path(&key));
                    anyhow::bail!(
                        "stream session '{}' has expired or was terminated; pass reset: true to start a new stream",
                        session_id
                    );
                }

                if !map.contains_key(&key) {
                    let disk_path = stream_session_disk_path(&key);
                    let mut restored = false;
                    if disk_path.is_file() {
                        let is_expired = if let Ok(meta) = std::fs::metadata(&disk_path) {
                            if let Ok(modified) = meta.modified() {
                                std::time::SystemTime::now()
                                    .duration_since(modified)
                                    .map(|d| d >= std::time::Duration::from_secs(300))
                                    .unwrap_or(false)
                            } else {
                                false
                            }
                        } else {
                            false
                        };
                        if is_expired {
                            let _ = std::fs::remove_file(&disk_path);
                            if let Ok(mut tombstones) = self.tombstones.lock() {
                                Self::insert_tombstone(&mut tombstones, key.clone());
                            }
                            anyhow::bail!(
                                "stream session '{}' has expired or was terminated; pass reset: true to start a new stream",
                                session_id
                            );
                        }

                        if map.len() >= MAX_ACTIVE_STREAM_SESSIONS {
                            anyhow::bail!(
                                "maximum concurrent active stream sessions limit ({}) reached; retry later or reset unused sessions",
                                MAX_ACTIVE_STREAM_SESSIONS
                            );
                        }

                        if let Ok(data) = std::fs::read_to_string(&disk_path) {
                            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&data) {
                                if let (Some(acc), Some(count)) =
                                    (val["accumulated"].as_str(), val["chunk_count"].as_u64())
                                {
                                    map.insert(
                                        key.clone(),
                                        StreamSession {
                                            key: key.clone(),
                                            accumulated: acc.to_string(),
                                            chunk_count: count as usize,
                                            last_activity: Instant::now(),
                                        },
                                    );
                                    restored = true;
                                }
                            }
                        }
                    }

                    if !restored {
                        anyhow::bail!(
                            "stream session '{}' not found or was evicted; pass reset: true to start a new stream",
                            session_id
                        );
                    }
                }
            }

            if !map.contains_key(&key) {
                if map.len() >= MAX_ACTIVE_STREAM_SESSIONS {
                    anyhow::bail!(
                        "maximum concurrent active stream sessions limit ({}) reached; retry later or reset unused sessions",
                        MAX_ACTIVE_STREAM_SESSIONS
                    );
                }
            }

            let current_len = map.get(&key).map(|s| s.accumulated.len()).unwrap_or(0);
            if current_len + chunk.len() > MAX_STREAM_SESSION_BYTES {
                map.remove(&key);
                if let Ok(mut tombstones) = self.tombstones.lock() {
                    Self::insert_tombstone(&mut tombstones, key.clone());
                }
                let _ = std::fs::remove_file(stream_session_disk_path(&key));
                anyhow::bail!(
                    "stream session accumulated buffer ({} bytes) exceeds maximum limit of {} bytes",
                    current_len + chunk.len(),
                    MAX_STREAM_SESSION_BYTES
                );
            }

            let session = map.entry(key.clone()).or_insert_with(|| StreamSession {
                key: key.clone(),
                accumulated: String::new(),
                chunk_count: 0,
                last_activity: Instant::now(),
            });

            session.accumulated.push_str(chunk);
            session.chunk_count += 1;
            session.last_activity = Instant::now();

            let disk_path = stream_session_disk_path(&key);
            let payload = serde_json::json!({
                "accumulated": &session.accumulated,
                "chunk_count": session.chunk_count,
            });
            let _ = write_private_session_file(&disk_path, &payload.to_string());

            (session.accumulated.clone(), session.chunk_count)
        };

        let accumulated_bytes = accumulated.len();

        let is_checkpoint = is_last
            || chunk.contains('\n')
            || chunk.contains(';')
            || chunk.contains('}');

        if !is_checkpoint {
            return Ok(StreamChunkResult {
                session_id: session_id.to_string(),
                chunk_index,
                accumulated_bytes,
                checkpoint_evaluated: false,
                intercepted: false,
                interception: None,
                final_report: None,
                summary: format!("buffered chunk {} ({} bytes)", chunk_index, accumulated_bytes),
            });
        }

        match validate_text(remote, root, file, &accumulated).await {
            Ok(report) => {
                // A later chunk can add imports, trait impls, declarations, and types that make
                // semantic errors in this prefix valid. Do not terminate generation until the
                // source unit is complete.
                let serious_intercept = if is_last {
                    report.hallucinations.first().cloned()
                } else {
                    None
                };

                if let Some(intercept) = serious_intercept {
                    self.remove_session_key(&key);
                    return Ok(StreamChunkResult {
                        session_id: session_id.to_string(),
                        chunk_index,
                        accumulated_bytes,
                        checkpoint_evaluated: true,
                        intercepted: true,
                        interception: Some(intercept.clone()),
                        final_report: Some(report),
                        summary: format!(
                            "intercepted {} at chunk {} (line {}:{})",
                            intercept.kind, chunk_index, intercept.line, intercept.col
                        ),
                    });
                }

                if is_last {
                    self.remove_session_key(&key);
                    let errors = report.errors;
                    if borrow_check && errors == 0 {
                        let (compiled_errors, compiled_out) =
                            crate::tools::compile_check(remote, root, &[(file.to_path_buf(), accumulated.clone())]).await?;
                        if compiled_errors > 0 {
                            return Ok(StreamChunkResult {
                                session_id: session_id.to_string(),
                                chunk_index,
                                accumulated_bytes,
                                checkpoint_evaluated: true,
                                intercepted: true,
                                interception: Some(HallucinationInterception {
                                    kind: HallucinationKind::BorrowCheckerError,
                                    symbol_or_target: None,
                                    message: compiled_out,
                                    line: 0,
                                    col: 0,
                                    suggestion: Some("Address compiler/borrow-checker violations".to_string()),
                                }),
                                final_report: Some(report),
                                summary: "borrow-checker / compiler verification rejected proposal".to_string(),
                            });
                        }
                    }

                    let passed = errors == 0;
                    return Ok(StreamChunkResult {
                        session_id: session_id.to_string(),
                        chunk_index,
                        accumulated_bytes,
                        checkpoint_evaluated: true,
                        intercepted: !passed,
                        interception: report.hallucinations.first().cloned(),
                        final_report: Some(report),
                        summary: if passed {
                            "stream generation validated clean: 0 hallucinations intercepted".to_string()
                        } else {
                            format!("stream completed with {errors} error(s)")
                        },
                    });
                }

                Ok(StreamChunkResult {
                    session_id: session_id.to_string(),
                    chunk_index,
                    accumulated_bytes,
                    checkpoint_evaluated: true,
                    intercepted: false,
                    interception: None,
                    final_report: None,
                    summary: if report.errors > 0 {
                        format!(
                            "checkpoint at chunk {} found {} provisional error(s); semantic interceptions are deferred until close",
                            chunk_index, report.errors
                        )
                    } else {
                        format!("checkpoint at chunk {} passed", chunk_index)
                    },
                })
            }
            Err(err) => {
                if is_last {
                    self.remove_session_key(&key);
                    Err(err)
                } else {
                    Ok(StreamChunkResult {
                        session_id: session_id.to_string(),
                        chunk_index,
                        accumulated_bytes,
                        checkpoint_evaluated: true,
                        intercepted: false,
                        interception: None,
                        final_report: None,
                        summary: format!("checkpoint at chunk {} skipped due to intermediate syntax: {err}", chunk_index),
                    })
                }
            }
        }
    }
}

/// Streamed syntax and type check verification during agent code generation (Phase 7.7).
/// Intercepts invalid method invocations, incorrect argument types, or borrow-checker errors
/// before the agent even finishes generating its turn, providing immediate feedback and
/// eliminating multi-turn debugging cycles.
pub async fn validate_stream_chunks(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    chunks: &[String],
    borrow_check: bool,
) -> Result<StreamValidationResult> {
    let count = BATCH_COUNTER.fetch_add(1, Ordering::Relaxed);
    let session_id = format!("batch-{}-{}", std::process::id(), count);
    let mgr = stream_manager();
    mgr.reset_session(remote, root, file, &session_id);
    let total_chunks = chunks.len();

    for (idx, chunk) in chunks.iter().enumerate() {
        let is_last = idx + 1 == total_chunks;
        let is_first = idx == 0;
        let res = mgr
            .feed_chunk(remote, root, file, &session_id, chunk, is_last, is_first, borrow_check)
            .await?;
        if res.intercepted {
            mgr.reset_session(remote, root, file, &session_id);
            return Ok(StreamValidationResult {
                completed_chunks: idx + 1,
                total_chunks,
                intercepted: true,
                interception: res.interception,
                intercept_chunk_index: Some(idx),
                final_report: res.final_report,
                summary: res.summary,
            });
        }
        if is_last {
            mgr.reset_session(remote, root, file, &session_id);
            return Ok(StreamValidationResult {
                completed_chunks: total_chunks,
                total_chunks,
                intercepted: false,
                interception: None,
                intercept_chunk_index: None,
                final_report: res.final_report,
                summary: res.summary,
            });
        }
    }

    mgr.reset_session(remote, root, file, &session_id);
    Ok(StreamValidationResult {
        completed_chunks: total_chunks,
        total_chunks,
        intercepted: false,
        interception: None,
        intercept_chunk_index: None,
        final_report: None,
        summary: "stream completed".to_string(),
    })
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
    let query_params = serde_json::json!({ "textDocument": { "uri": uri } });
    let result = match session
        .query(file, "textDocument/diagnostic", query_params.clone())
        .await
    {
        Ok(r) => r,
        Err(err) => {
            tracing::warn!(
                error = %err,
                file = %file.display(),
                "initial on_disk diagnostic query failed, retrying once after backoff"
            );
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            session
                .query(file, "textDocument/diagnostic", query_params)
                .await
                .ok()?
        }
    };
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
    for file in edits.iter().map(|(file, _)| file).chain(also_check) {
        if !is_parser_validated_file(file) {
            ensure_source_file(file)?;
        }
    }
    // An extra file is checked against its text on disk; one that cannot be read would come back
    // as a clean report nobody made, and the change would pass unchecked there (#446).
    // Read and verify all also_check files up-front to fail closed in all branches.
    let mut also_texts = Vec::with_capacity(also_check.len());
    for file in also_check {
        let abs = if file.is_absolute() {
            file.clone()
        } else {
            root.join(file)
        };
        also_texts.push(std::fs::read_to_string(&abs).map_err(|e| {
            anyhow::anyhow!(
                "cannot read {}, which the change must be checked against; nothing was \
                 validated: {e}",
                abs.display()
            )
        })?);
    }
    // Fast path: if all files are parser-validated, validate locally without an LSP session (#733, #778).
    if edits.iter().all(|(f, _)| is_parser_validated_file(f))
        && also_check.iter().all(|f| is_parser_validated_file(f))
    {
        let mut reports = Vec::with_capacity(edits.len() + also_check.len());
        for (file, text) in edits {
            reports.push(validate_file_content(&display(root, file), file, text));
        }
        for (file, text) in also_check.iter().zip(&also_texts) {
            reports.push(validate_file_content(&display(root, file), file, text));
        }
        return Ok(reports);
    }

    let engines: HashSet<&'static str> = edits
        .iter()
        .map(|(p, _)| crate::lang::engine_group_for_path(p))
        .chain(also_check.iter().map(|p| crate::lang::engine_group_for_path(p)))
        .collect();

    // When a batch spans multiple languages, route each subset to its matching LSP engine
    // rather than asking one engine (e.g. gopls) for diagnostics on incompatible files (#751, #761).
    if engines.len() > 1 {
        let mut final_reports: Vec<Option<DiagnosticsReport>> =
            vec![None; edits.len() + also_check.len()];

        for &engine in &engines {
            if engine == "json" || engine == "markdown" || engine == "xml" {
                for (orig_i, (file, text)) in edits.iter().enumerate() {
                    if crate::lang::engine_group_for_path(file) == engine {
                        final_reports[orig_i] =
                            Some(validate_file_content(&display(root, file), file, text));
                    }
                }
                for (orig_j, file) in also_check.iter().enumerate() {
                    if crate::lang::engine_group_for_path(file) == engine {
                        final_reports[edits.len() + orig_j] = Some(validate_file_content(
                            &display(root, file),
                            file,
                            &also_texts[orig_j],
                        ));
                    }
                }
                continue;
            }

            let mut group_edits = Vec::new();
            let mut edit_indices = Vec::new();
            for (orig_i, edit) in edits.iter().enumerate() {
                if crate::lang::engine_group_for_path(&edit.0) == engine {
                    group_edits.push(edit.clone());
                    edit_indices.push(orig_i);
                }
            }

            let mut group_also = Vec::new();
            let mut group_also_texts = Vec::new();
            let mut also_indices = Vec::new();
            for (orig_j, file) in also_check.iter().enumerate() {
                if crate::lang::engine_group_for_path(file) == engine {
                    group_also.push(file.clone());
                    group_also_texts.push(also_texts[orig_j].clone());
                    also_indices.push(orig_j);
                }
            }

            if group_edits.is_empty() && group_also.is_empty() {
                continue;
            }

            let sub_reports = Box::pin(validate_texts_single_engine(
                remote,
                root,
                &group_edits,
                &group_also,
                &group_also_texts,
            ))
            .await?;

            for (sub_i, &orig_i) in edit_indices.iter().enumerate() {
                final_reports[orig_i] = Some(sub_reports[sub_i].clone());
            }
            for (sub_j, &orig_j) in also_indices.iter().enumerate() {
                final_reports[edits.len() + orig_j] =
                    Some(sub_reports[group_edits.len() + sub_j].clone());
            }
        }

        return Ok(final_reports
            .into_iter()
            .enumerate()
            .map(|(idx, r)| {
                r.unwrap_or_else(|| {
                    let file = if idx < edits.len() {
                        &edits[idx].0
                    } else {
                        &also_check[idx - edits.len()]
                    };
                    DiagnosticsReport {
                        file: display(root, file),
                        errors: 0,
                        warnings: 0,
                        items: Vec::new(),
                        preexisting: Vec::new(),
                        in_derive: Vec::new(),
                        auto_trait: Vec::new(),
                        hallucinations: Vec::new(),
                    }
                })
            })
            .collect());
    }

    validate_texts_single_engine(remote, root, edits, also_check, &also_texts).await
}

async fn validate_texts_single_engine(
    remote: SocketAddr,
    root: &Path,
    edits: &[(std::path::PathBuf, String)],
    also_check: &[std::path::PathBuf],
    also_texts: &[String],
) -> Result<Vec<DiagnosticsReport>> {
    let hint = edits
        .first()
        .map(|(file, _)| file.as_path())
        .or_else(|| also_check.first().map(|p| p.as_path()));
    // What every file says as it is on disk: an error the checkout already has is not the
    // edit's, and a report that counts it refuses every edit to that file.
    let mut baselines: HashMap<String, (DiagnosticsReport, String)> = HashMap::new();
    let mut order: Vec<usize> = (0..edits.len()).collect();
    order.sort_by_key(|&i| !crate::lang::is_header(&edits[i].0));

    // Rust discovers module contents through their parent file. Open nested source files first
    // so a newly added child exists in the overlay before its parent re-export is analyzed.
    let mut rust_order: Vec<usize> = order
        .iter()
        .copied()
        .filter(|&i| edits[i].0.extension().and_then(|ext| ext.to_str()) == Some("rs"))
        .collect();
    rust_order.sort_by_key(|&i| {
        let file = &edits[i].0;
        let is_module_root = matches!(
            file.file_name().and_then(|name| name.to_str()),
            Some("lib.rs" | "main.rs" | "mod.rs")
        );
        std::cmp::Reverse(file.components().count().saturating_sub(if is_module_root {
            1
        } else {
            0
        }))
    });
    let mut rust_order = rust_order.into_iter();
    for i in &mut order {
        if edits[*i].0.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            *i = rust_order
                .next()
                .expect("one Rust edit per Rust order slot");
        }
    }

    // Capture the pre-edit symbols against the checkout, on the validation engine's unchanged
    // view. This has to finish before opening any proposal, since a new child module can change
    // what documentSymbol reports for its parent.
    let mut before_symbols = Vec::with_capacity(order.len());
    {
        let mut checkout = LspSession::open_for_validation(remote, root, hint).await?;
        for file in edits.iter().map(|(f, _)| f).chain(also_check) {
            let shown = display(root, file);
            if let Some(before) = on_disk(&mut checkout, root, file, &shown).await {
                baselines.insert(shown, before);
            }
        }
        for &i in &order {
            let file = &edits[i].0;
            if root.join(file).is_file() || file.is_file() {
                let uri = checkout.uri_for(file)?;
                before_symbols.push(
                    checkout
                        .query(
                            file,
                            "textDocument/documentSymbol",
                            serde_json::json!({ "textDocument": { "uri": uri } }),
                        )
                        .await
                        .map(|r| symbol_names(&r))
                        .unwrap_or_default(),
                );
            } else {
                before_symbols.push(BTreeSet::new());
            }
        }
        checkout.close().await;
    }

    let mut session = LspSession::open_for_validation(remote, root, hint).await?;
    let mut sources: HashMap<String, String> = HashMap::new();
    // clangd builds a source against the header text that is open when the source is built.
    // Open headers first so every source sees its proposed header (#292); then open every file
    // before asking for any after-set so new nested modules are visible to their parents.
    let mut proposed = Vec::with_capacity(order.len());
    for &i in &order {
        let (file, text) = &edits[i];
        let uri = session.open_text(file, text).await?;
        sources.insert(display(root, file), text.clone());
        proposed.push((i, file.clone(), uri));
    }

    // Compare only after the complete proposal is open. A re-export from a proposed child file
    // must not look removed just because that child was not open when the parent was queried.
    let mut missing: Vec<(String, String)> = Vec::new();
    for ((_, file, uri), before) in proposed.iter().zip(before_symbols) {
        let after = session
            .request(
                "textDocument/documentSymbol",
                serde_json::json!({ "textDocument": { "uri": uri } }),
            )
            .await
            .map(|r| symbol_names(&r))
            .unwrap_or_default();
        let shown = display(root, file);
        for name in before.difference(&after) {
            missing.push((name.clone(), shown.clone()));
        }
    }

    // The proposal was opened in dependency order; reports keep the caller's edit order.
    let mut diagnostic_order: Vec<_> = proposed.iter().collect();
    diagnostic_order.sort_by_key(|(i, _, _)| *i);
    let mut reports = Vec::with_capacity(edits.len() + also_check.len());
    for (i, file, uri) in diagnostic_order {
        let diag_result = session
            .request(
                "textDocument/diagnostic",
                serde_json::json!({ "textDocument": { "uri": uri } }),
            )
            .await;
        let result = match diag_result {
            Ok(res) => res,
            Err(err) => {
                if let Some(reason) = platform_exclusion_reason(file, &err) {
                    let shown = display(root, file);
                    let report = platform_excluded_report(&shown, &reason);
                    debug_assert_eq!(reports.len(), *i);
                    reports.push(report);
                    continue;
                }
                return Err(err);
            }
        };
        let shown = display(root, file);
        let mut report = parse_items(&shown, &result);
        if let (Some((before, before_text)), Some(text)) =
            (baselines.get(&shown), sources.get(&shown))
        {
            set_aside_preexisting(&mut report, text, before, before_text);
        }
        if let Some(text) = sources.get(&shown) {
            set_aside_derive_expansions(&mut report, text);
        }
        refuse_unchecked(&mut report);
        debug_assert_eq!(reports.len(), *i);
        reports.push(report);
    }
    for (file, text) in also_check.iter().zip(also_texts) {
        let uri = session.uri_for(file)?;
        let diag_result = session
            .query(
                file,
                "textDocument/diagnostic",
                serde_json::json!({ "textDocument": { "uri": uri } }),
            )
            .await;
        let result = match diag_result {
            Ok(res) => res,
            Err(err) => {
                if let Some(reason) = platform_exclusion_reason(file, &err) {
                    let shown = display(root, file);
                    let report = platform_excluded_report(&shown, &reason);
                    reports.push(report);
                    continue;
                }
                return Err(err);
            }
        };
        let shown = display(root, file);
        let mut report = parse_items(&shown, &result);
        if let Some((before, before_text)) = baselines.get(&shown) {
            set_aside_preexisting(&mut report, text, before, before_text);
        }
        set_aside_derive_expansions(&mut report, text);
        refuse_unchecked(&mut report);
        sources.insert(shown.clone(), text.clone());
        reports.push(report);
    }
    suppress_used_public_reexport_warnings(&mut session, root, edits.len(), &mut reports, &sources)
        .await;
    let resolved = resolved_tokens(&mut session, root, &reports, &sources, &missing).await;
    session.close().await;
    annotate_missing_symbols(&mut reports, &sources, &missing, &resolved);
    let _ = reconcile_swift_cross_target_diagnostics(remote, root, edits, &mut reports).await;
    Ok(reports)
}

/// Returns a reason string if the error indicates the file was excluded by platform build tags or constraints.
pub fn platform_exclusion_reason(file: &Path, err: &anyhow::Error) -> Option<String> {
    let err_str = err.to_string();
    let lower_err = err_str.to_lowercase();
    let file_str = file.to_string_lossy();

    let is_metadata_err = lower_err.contains("no package metadata")
        || lower_err.contains("no packages found")
        || lower_err.contains("build constraints exclude all go files")
        || lower_err.contains("cannot find package")
        || lower_err.contains("no such package");

    if is_metadata_err {
        if file_str.ends_with("_darwin.go") {
            return Some("Darwin / macOS build tags excluded on current host".to_string());
        }
        if file_str.ends_with("_linux.go") {
            return Some("Linux build tags excluded on current host".to_string());
        }
        if file_str.ends_with("_windows.go") {
            return Some("Windows build tags excluded on current host".to_string());
        }
        if file_str.ends_with("_freebsd.go") || file_str.ends_with("_openbsd.go") {
            return Some("BSD build tags excluded on current host".to_string());
        }
        return Some(format!(
            "excluded by platform build constraints: {}",
            err_str.lines().next().unwrap_or(&err_str)
        ));
    }
    None
}

pub fn platform_excluded_report(file: &str, reason: &str) -> DiagnosticsReport {
    DiagnosticsReport {
        file: file.to_string(),
        errors: 0,
        warnings: 0,
        items: vec![DocDiagnostic {
            severity: "info".to_string(),
            code: Some("platform-excluded".to_string()),
            message: reason.to_string(),
            line: 1,
            col: 1,
            source: Some("prod-code".to_string()),
            note: None,
            end: None,
        }],
        preexisting: Vec::new(),
        in_derive: Vec::new(),
        auto_trait: Vec::new(),
        hallucinations: Vec::new(),
    }
}

/// Whether an error message matches a Swift compiler symbol or type resolution error across targets.
pub fn is_swift_cross_target_candidate(msg: &str) -> bool {
    let lower = msg.to_lowercase();
    lower.contains("has no member")
        || lower.contains("cannot find type")
        || lower.contains("cannot find '")
        || lower.contains("no member named")
        || lower.contains("is not a member type of")
        || lower.contains("extra argument")
        || lower.contains("incorrect argument label")
        || lower.contains("missing argument for parameter")
        || lower.contains("do not match any available overload")
}

/// The target name of a Swift file based on SwiftPM standard layout (Sources/<Target> or Tests/<Target>).
pub fn swift_target_name(path: &Path) -> Option<String> {
    let parts: Vec<&str> = path
        .components()
        .map(|c| c.as_os_str().to_str().unwrap_or(""))
        .collect();
    for (i, &p) in parts.iter().enumerate() {
        if (p == "Sources" || p == "Tests") && i + 1 < parts.len() {
            return Some(parts[i + 1].to_string());
        }
    }
    None
}

/// Swift sourcekit-lsp cannot compile dependent targets in-memory across module boundaries;
/// when target B imports target A with @testable or import, sourcekit-lsp evaluates target B
/// against the on-disk .swiftmodule in .build, which may be stale or lack proposed in-memory
/// members. When errors in a Swift validation batch match cross-target resolution failures or
/// span multiple targets, verify them with a shadow compiler check (`swift build --build-tests`).
/// If the compiler accepts the overlay with zero errors, suppress the false positive diagnostics (#759, #762).
async fn reconcile_swift_cross_target_diagnostics(
    remote: SocketAddr,
    root: &Path,
    edits: &[(std::path::PathBuf, String)],
    reports: &mut [DiagnosticsReport],
) -> Result<()> {
    let has_swift = edits
        .iter()
        .any(|(p, _)| p.extension().and_then(|e| e.to_str()) == Some("swift"));
    if !has_swift {
        return Ok(());
    }

    let has_errors = reports.iter().any(|r| r.errors > 0);
    if !has_errors {
        return Ok(());
    }

    let targets: HashSet<String> = edits
        .iter()
        .filter_map(|(p, _)| swift_target_name(p))
        .collect();
    let is_cross_target = targets.len() > 1;
    let has_candidate_error = reports.iter().any(|r| {
        r.items
            .iter()
            .any(|i| i.severity == "error" && is_swift_cross_target_candidate(&i.message))
    });

    if !is_cross_target && !has_candidate_error {
        return Ok(());
    }

    if let Ok((0, _output)) = crate::tools::compile_check(remote, root, edits).await {
        tracing::info!(
            "Swift cross-target overlay verified cleanly by shadow compiler; suppressing stale sourcekit-lsp diagnostics"
        );
        for report in reports.iter_mut() {
            report.items.retain(|item| {
                item.severity != "error" || !is_swift_cross_target_candidate(&item.message)
            });
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
            refresh_hallucinations(report);
        }
    }

    Ok(())
}

/// A re-export reference in a checked caller proves that a public `pub use` is used, even
/// when rust-analyzer tags the import as unused. Suppress only when the references request at the
/// re-export token returns a location in an unchanged checked caller; unresolved or unreferenced
/// imports remain diagnostics.
async fn suppress_used_public_reexport_warnings(
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

fn public_reexport_token(text: &str, diagnostic: &DocDiagnostic) -> Option<u32> {
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
        vec![statement
            .rsplit_once(" as ")
            .map(|(_, alias)| alias.trim())
            .unwrap_or_else(|| statement.rsplit("::").next().unwrap_or(statement).trim())]
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
async fn resolved_tokens(
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

fn has_definition(value: &serde_json::Value) -> bool {
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
            { "range": { "start": { "line": 3, "character": 4 }, "end": { "line": 3, "character": 5 } }, "severity": 4,
              "code": "inactive-code", "message": "code is inactive due to #[cfg] directives: unix is enabled" },
            { "range": { "start": { "line": 10, "character": 8 }, "end": { "line": 10, "character": 9 } }, "severity": 1,
              "code": "E0425", "message": "cannot find value `x` in this scope" },
            { "range": { "start": { "line": 12, "character": 1 }, "end": { "line": 12, "character": 2 } }, "severity": 4,
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
        // A function's `let` bindings are listed as variables and are nobody else's to use.
        let with_locals = serde_json::json!([
            { "name": "run_safe_delete", "kind": 12, "children": [
                { "name": "edit", "kind": 13 }, { "name": "touched", "kind": 13 }
            ] },
            { "name": "LIMIT", "kind": 14 }
        ]);
        assert_eq!(
            symbol_names(&with_locals).into_iter().collect::<Vec<_>>(),
            vec!["LIMIT".to_string(), "run_safe_delete".to_string()]
        );
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
            end: None,
        }
    }

    fn report_of(items: Vec<DocDiagnostic>) -> DiagnosticsReport {
        DiagnosticsReport {
            file: "src/messages.rs".to_string(),
            errors: items.iter().filter(|d| d.severity == "error").count(),
            warnings: items.iter().filter(|d| d.severity == "warning").count(),
            items,
            preexisting: vec![],
            in_derive: vec![],
            auto_trait: vec![],
            hallucinations: vec![],
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
    fn a_preexisting_error_is_removed_from_stream_interceptions() {
        let text = "fn call() { value.missing(); }\n";
        let error = DocDiagnostic {
            code: Some("unresolved-method".to_string()),
            ..diagnostic("error", "no method named `missing` found", 1)
        };
        let before = report_of(vec![error.clone()]);
        let mut report = report_of(vec![error]);
        report.hallucinations = report.items.iter().filter_map(classify_hallucination).collect();
        assert_eq!(report.hallucinations.len(), 1);

        set_aside_preexisting(&mut report, text, &before, text);

        assert!(report.items.is_empty());
        assert_eq!(report.errors, 0);
        assert!(report.hallucinations.is_empty());
    }

    #[test]
    fn inference_failing_in_a_derive_expansion_is_not_counted_even_in_a_new_file() {
        let text = "use serde::Deserialize;\n\n#[derive(Debug, Deserialize)]\npub struct P {\n    pub a: u32,\n}\nfn f() -> u8 { \"\" }\n";
        let mut report = report_of(vec![
            DocDiagnostic {
                code: Some("E0282".into()),
                ..diagnostic("error", "type annotations needed", 3)
            },
            // Anything else on a derive line, or E0282 anywhere else, still counts.
            DocDiagnostic {
                code: Some("E0277".into()),
                ..diagnostic("error", "the trait bound is not satisfied", 3)
            },
            DocDiagnostic {
                code: Some("E0282".into()),
                ..diagnostic("error", "type annotations needed", 7)
            },
        ]);
        set_aside_derive_expansions(&mut report, text);
        assert_eq!(report.in_derive.len(), 1);
        let lines: Vec<(u32, Option<&str>)> = report
            .items
            .iter()
            .map(|d| (d.line, d.code.as_deref()))
            .collect();
        assert_eq!(lines, [(3, Some("E0277")), (7, Some("E0282"))]);
        assert_eq!(report.errors, 2);
        assert!(
            report
                .render()
                .contains("1 \"type annotations needed\" on a #[derive(...)] line are not counted"),
            "{}",
            report.render()
        );
    }

    /// An unproven `Send` bound in a Rust file is shown and not counted (#327): rust-analyzer
    /// does not prove it through a recursive `async fn`, where rustc does. Other E0277, and the
    /// same message in a file of another language, still count.
    #[test]
    fn an_unproven_auto_trait_bound_is_shown_and_not_counted() {
        let text = "fn a() {}\nfn b() {}\nfn c() {}\n";
        let mut report = report_of(vec![
            DocDiagnostic {
                code: Some("E0277".into()),
                ..diagnostic(
                    "error",
                    "the trait bound `NonNull<()>: Send` is not satisfied",
                    1,
                )
            },
            DocDiagnostic {
                code: Some("E0277".into()),
                ..diagnostic(
                    "error",
                    "`Rc<u8>` cannot be shared between threads safely",
                    2,
                )
            },
            DocDiagnostic {
                code: Some("E0277".into()),
                ..diagnostic("error", "the trait bound `u8: Display` is not satisfied", 3)
            },
        ]);
        report.file = "src/main.rs".into();
        set_aside_derive_expansions(&mut report, text);
        assert_eq!(report.auto_trait.len(), 2);
        assert_eq!(report.errors, 1);
        let rendered = report.render();
        assert!(
            rendered.contains("2 unproven Send/Sync/Unpin bound(s) are not counted")
                && rendered.contains("unconfirmed: the trait bound `NonNull<()>: Send` is not satisfied (src/main.rs:1:"),
            "{rendered}"
        );
        assert!(is_auto_trait_bound(
            "`Rc<u8>` cannot be sent between threads safely"
        ));
        let mut other = report_of(vec![DocDiagnostic {
            code: Some("E0277".into()),
            ..diagnostic(
                "error",
                "the trait bound `NonNull<()>: Send` is not satisfied",
                1,
            )
        }]);
        other.file = "main.swift".into();
        set_aside_derive_expansions(&mut other, text);
        assert_eq!(other.errors, 1, "only a Rust file's");
    }

    #[test]
    fn a_file_the_analyzer_could_not_check_is_never_set_aside() {
        let text = "fn a() {}\n";
        let panic = DocDiagnostic {
            code: Some(prod_code_protocol::ANALYZER_PANIC_CODE.to_string()),
            ..diagnostic(
                "error",
                "rust-analyzer panicked while checking this file",
                1,
            )
        };
        let before = report_of(vec![panic.clone()]);
        let mut report = report_of(vec![panic]);
        set_aside_preexisting(&mut report, text, &before, text);
        assert!(report.preexisting.is_empty());
        assert_eq!(
            report.errors, 1,
            "still an error: nothing in the file was checked"
        );
    }

    fn unlinked(severity: &str) -> DocDiagnostic {
        DocDiagnostic {
            code: Some(UNLINKED_FILE.to_string()),
            ..diagnostic(
                severity,
                "This file is not included in any crates, so rust-analyzer can't offer IDE services.",
                1,
            )
        }
    }

    /// The analyzer's `unlinked-file` hint in a proposal is an error with a note, and the message
    /// stays the analyzer's: nothing claims the file has a type error (#467).
    #[test]
    fn an_unlinked_proposal_is_counted_and_explained() {
        let other = DocDiagnostic {
            code: Some("unused_variables".into()),
            ..diagnostic("hint", "unused variable", 3)
        };
        let mut report = report_of(vec![unlinked("hint"), other]);
        report.file = "tests/new.rs".into();
        assert!(report.ok(), "the analyzer's answer alone counts nothing");
        refuse_unchecked(&mut report);
        assert!(!report.ok());
        assert_eq!((report.errors, report.warnings), (1, 0));
        let item = &report.items[0];
        assert_eq!(item.severity, "error");
        assert!(
            item.message
                .starts_with("This file is not included in any crates")
        );
        let note = item.note.as_deref().unwrap();
        assert!(
            note.contains("not type-checked")
                && note.contains("not because an error was found")
                && note.contains("--all-targets"),
            "{note}"
        );
        assert_eq!(
            report.items[1].severity, "hint",
            "other hints are left alone"
        );
        assert!(report.items[1].note.is_none());
        let rendered = report.render();
        assert!(
            rendered.contains("tests/new.rs: 1 error(s), 0 warning(s)")
                && rendered.contains("error: This file is not included in any crates")
                && rendered.contains("[unlinked-file] (tests/new.rs:1:3)")
                && rendered.contains("note: rust-analyzer includes this file in no crate"),
            "{rendered}"
        );
    }

    /// Control: a report without `unlinked-file` keeps its items and counts.
    #[test]
    fn a_linked_report_is_left_as_it_is() {
        let mut report = report_of(vec![
            diagnostic("warning", "type annotations needed", 2),
            DocDiagnostic {
                code: Some("unused_variables".into()),
                ..diagnostic("hint", "unused variable", 3)
            },
        ]);
        refuse_unchecked(&mut report);
        assert!(report.ok());
        assert_eq!((report.errors, report.warnings), (0, 1));
        assert!(report.items.iter().all(|d| d.note.is_none()));
    }

    /// A file already unlinked on disk was unchecked before the edit too; that does not set the
    /// proposal's `unlinked-file` aside.
    #[test]
    fn an_unlinked_file_is_never_set_aside() {
        let text = "pub fn helper() -> u32 {\n    1\n}\n";
        let before = report_of(vec![unlinked("hint")]);
        let mut report = report_of(vec![unlinked("hint")]);
        set_aside_preexisting(&mut report, text, &before, text);
        assert!(report.preexisting.is_empty(), "{:?}", report.preexisting);
        refuse_unchecked(&mut report);
        assert_eq!(report.errors, 1);
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
                    end: None,
                },
                DocDiagnostic {
                    severity: "hint".to_string(),
                    code: None,
                    message: "unused".to_string(),
                    line: 3,
                    col: 1,
                    source: None,
                    note: None,
                    end: None,
                },
            ],
            preexisting: vec![],
            in_derive: vec![],
            auto_trait: vec![],
            hallucinations: vec![],
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
        annotate_missing_symbols(&mut reports, &sources, &missing, &BTreeSet::new());
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
                in_derive: vec![],
                auto_trait: vec![],
                hallucinations: vec![],
            },
            DiagnosticsReport {
                file: "crates/gateway/src/workspace.rs".to_string(),
                errors: 0,
                warnings: 0,
                items: vec![],
                preexisting: vec![],
                in_derive: vec![],
                auto_trait: vec![],
                hallucinations: vec![],
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
        annotate_missing_symbols(&mut reports, &sources, &missing, &BTreeSet::new());
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

    #[test]
    fn doc_and_line_comments_mentioning_removed_symbol_are_not_flagged() {
        let mut reports = vec![DiagnosticsReport {
            file: "crates/gateway/src/workspace.rs".to_string(),
            errors: 0,
            warnings: 0,
            items: vec![],
            preexisting: vec![],
            in_derive: vec![],
            auto_trait: vec![],
            hallucinations: vec![],
        }];
        let mut sources = HashMap::new();
        sources.insert(
            "crates/gateway/src/workspace.rs".to_string(),
            "/// covers `excess` bytes. Only engines without a session\n\
//! covers the whole tree\n\
// ordinary line comment with covers\n\
pub fn work() {}\n"
                .to_string(),
        );
        let missing = vec![("covers".to_string(), "crates/engine/src/lib.rs".to_string())];
        annotate_missing_symbols(&mut reports, &sources, &missing, &BTreeSet::new());
        assert_eq!(reports[0].warnings, 0, "no false warnings on comments");
        assert!(reports[0].items.is_empty(), "{:?}", reports[0].items);

        let mut reports_with_warning = vec![DiagnosticsReport {
            file: "crates/gateway/src/workspace.rs".to_string(),
            errors: 0,
            warnings: 1,
            items: vec![DocDiagnostic {
                severity: "warning".to_string(),
                code: Some("dead_code".to_string()),
                message: "unused".to_string(),
                line: 1,
                col: 1,
                source: None,
                note: None,
                end: None,
            }],
            preexisting: vec![],
            in_derive: vec![],
            auto_trait: vec![],
            hallucinations: vec![],
        }];
        annotate_missing_symbols(
            &mut reports_with_warning,
            &sources,
            &missing,
            &BTreeSet::new(),
        );
        assert!(reports_with_warning[0].items[0].note.is_none());
    }

    #[test]
    fn block_comments_nested_and_multiline_are_not_flagged() {
        let mut reports = vec![DiagnosticsReport {
            file: "crates/gateway/src/workspace.rs".to_string(),
            errors: 0,
            warnings: 0,
            items: vec![],
            preexisting: vec![],
            in_derive: vec![],
            auto_trait: vec![],
            hallucinations: vec![],
        }];
        let mut sources = HashMap::new();
        sources.insert(
            "crates/gateway/src/workspace.rs".to_string(),
            "/*\n * multiline comment\n * covers\n */\n\
/* outer /* inner covers */ still comment */\n\
/** doc block comment covers */\n\
/*! inner doc block covers */\n\
pub fn work() {}\n"
                .to_string(),
        );
        let missing = vec![("covers".to_string(), "crates/engine/src/lib.rs".to_string())];
        annotate_missing_symbols(&mut reports, &sources, &missing, &BTreeSet::new());
        assert_eq!(reports[0].warnings, 0);
        assert!(reports[0].items.is_empty());
    }

    #[test]
    fn strings_and_literals_are_not_flagged_as_stale_references() {
        let mut reports = vec![DiagnosticsReport {
            file: "crates/gateway/src/workspace.rs".to_string(),
            errors: 0,
            warnings: 0,
            items: vec![],
            preexisting: vec![],
            in_derive: vec![],
            auto_trait: vec![],
            hallucinations: vec![],
        }];
        let mut sources = HashMap::new();
        sources.insert(
            "crates/gateway/src/workspace.rs".to_string(),
            "fn run() {\n\
    let a = \"covers\";\n\
    let b = \"multiline \\\n             covers string\";\n\
    let c = \"escaped \\\" covers \\\"\";\n\
    let d = r#\"raw covers string\"#;\n\
    let e = r##\"nested # raw covers string\"##;\n\
    let f = b\"byte covers\";\n\
    let g = br#\"raw byte covers\"#;\n\
    let h = c\"c string covers\";\n\
    let i = 'c';\n\
    let j = '\\'';\n\
    let k = b'\\n';\n\
}\n\
fn lifetime<'covers>(x: &'covers str) -> &'covers str { x }\n"
                .to_string(),
        );
        let missing = vec![("covers".to_string(), "crates/engine/src/lib.rs".to_string())];
        annotate_missing_symbols(&mut reports, &sources, &missing, &BTreeSet::new());
        assert_eq!(reports[0].warnings, 0, "{:?}", reports[0].items);
        assert!(reports[0].items.is_empty());
    }

    #[test]
    fn real_code_tokens_attach_at_token_position_even_with_preceding_prose() {
        let mut reports = vec![DiagnosticsReport {
            file: "crates/gateway/src/workspace.rs".to_string(),
            errors: 0,
            warnings: 0,
            items: vec![],
            preexisting: vec![],
            in_derive: vec![],
            auto_trait: vec![],
            hallucinations: vec![],
        }];
        let mut sources = HashMap::new();
        sources.insert(
            "crates/gateway/src/workspace.rs".to_string(),
            r#"fn run() {
    /* covers comment */ workspace::covers(&ws);
    let msg = "covers in string"; covers();
    r#covers();
    self.covers();
    // 🦀 covers emoji
    let 🦀 = covers();
}
"#
            .to_string(),
        );
        let missing = vec![("covers".to_string(), "crates/engine/src/lib.rs".to_string())];
        annotate_missing_symbols(&mut reports, &sources, &missing, &BTreeSet::new());
        assert_eq!(reports[0].warnings, 5, "{:?}", reports[0].items);

        let d0 = &reports[0].items[0];
        assert_eq!(d0.line, 2);
        assert_eq!(d0.col, 37);

        let d1 = &reports[0].items[1];
        assert_eq!(d1.line, 3);
        assert_eq!(d1.col, 35);

        let d2 = &reports[0].items[2];
        assert_eq!(d2.line, 4);
        assert_eq!(d2.col, 5);

        let d3 = &reports[0].items[3];
        assert_eq!(d3.line, 5);
        assert_eq!(d3.col, 10);

        let d4 = &reports[0].items[4];
        assert_eq!(d4.line, 7);
        assert_eq!(d4.col, 14);
    }

    #[test]
    fn whole_identifier_boundaries_do_not_flag_substring_matches() {
        let mut reports = vec![DiagnosticsReport {
            file: "crates/gateway/src/workspace.rs".to_string(),
            errors: 0,
            warnings: 0,
            items: vec![],
            preexisting: vec![],
            in_derive: vec![],
            auto_trait: vec![],
            hallucinations: vec![],
        }];
        let mut sources = HashMap::new();
        sources.insert(
            "crates/gateway/src/workspace.rs".to_string(),
            "fn run() {\n\
    undercovers();\n\
    covers_everything();\n\
    my_covers_fn();\n\
}\n"
                .to_string(),
        );
        let missing = vec![("covers".to_string(), "crates/engine/src/lib.rs".to_string())];
        annotate_missing_symbols(&mut reports, &sources, &missing, &BTreeSet::new());
        assert_eq!(reports[0].warnings, 0);
        assert!(reports[0].items.is_empty());
    }

    #[test]
    fn non_rust_files_preserve_existing_behavior_and_utf16_columns() {
        let mut reports = vec![DiagnosticsReport {
            file: "gateway/workspace.go".to_string(),
            errors: 0,
            warnings: 0,
            items: vec![],
            preexisting: vec![],
            in_derive: vec![],
            auto_trait: vec![],
            hallucinations: vec![],
        }];
        let mut sources = HashMap::new();
        sources.insert(
            "gateway/workspace.go".to_string(),
            "package main\nfunc run() {\n    // 🚀 covers()\n}\n".to_string(),
        );
        let missing = vec![("covers".to_string(), "engine/lib.go".to_string())];
        annotate_missing_symbols(&mut reports, &sources, &missing, &BTreeSet::new());
        assert_eq!(reports[0].warnings, 1);
        assert_eq!(reports[0].items[0].line, 3);
        assert_eq!(reports[0].items[0].col, 11);
    }
}

#[cfg(test)]
mod identifier_boundary_regressions {
    #[test]
    fn combining_marks_remain_in_the_identifier() {
        let tokens = super::rust_code_identifiers(
            "fn run() { covers\u{0301}(); r#covers\u{0301}(); covers(); }",
        );
        let names: Vec<_> = tokens.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            ["fn", "run", "covers\u{0301}", "covers\u{0301}", "covers"]
        );
        assert!(!super::mentions_identifier("covers()", ""));
    }
}

#[cfg(test)]
mod definition_evidence_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn only_complete_definition_locations_prove_resolution() {
        let range = json!({"start":{"line":0,"character":7},"end":{"line":0,"character":13}});
        let location = json!({"uri":"file:///tmp/decl.rs","range":range});
        assert!(has_definition(&location));
        assert!(has_definition(&json!([location])));
        assert!(has_definition(
            &json!({"targetUri":"file:///tmp/decl.rs","targetSelectionRange":range})
        ));
        for value in [
            json!(null),
            json!([]),
            json!({}),
            json!([location, null]),
            json!({"uri":"file:///tmp/decl.rs","range":range,"error":{"code":-1}}),
            json!({"uri":"relative.rs","range":range}),
            json!({"uri":"file:///tmp/decl.rs"}),
            json!({"uri":"file:///tmp/decl.rs","range":{"start":{"line":1,"character":1},"end":{"line":0,"character":1}}}),
            json!({"uri":"file:///tmp/decl.rs","range":{"start":{"line":0,"character":4294967296u64},"end":{"line":0,"character":4294967296u64}}}),
        ] {
            assert!(!has_definition(&value), "{value}");
        }
    }

    #[tokio::test]
    async fn validate_texts_fails_closed_on_unreadable_also_check() {
        let root = Path::new("/nonexistent");
        let remote = "127.0.0.1:9400".parse().unwrap();
        let edits = vec![(std::path::PathBuf::from("valid.json"), "{}".to_string())];
        let also_check = vec![std::path::PathBuf::from("missing.json")];
        let result = validate_texts(remote, root, &edits, &also_check).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("cannot read"), "unexpected error: {err}");
    }
}

#[cfg(test)]
mod hallucination_interception_phase77_tests {
    use super::*;

    #[test]
    fn borrow_checker_error_codes_identified() {
        for code in &[
            "E0382", "E0499", "E0502", "E0503", "E0505", "E0506", "E0507", "E0515", "E0521",
            "E0596", "E0597", "E0716",
        ] {
            assert!(
                is_borrow_checker_error_code(code),
                "expected {code} to be borrow-checker error"
            );
        }
        for non_borrow in &["E0308", "E0425", "E0061", "E0277", "syntax-error"] {
            assert!(
                !is_borrow_checker_error_code(non_borrow),
                "expected {non_borrow} not to be borrow-checker error"
            );
        }
    }

    #[test]
    fn polyglot_method_name_extraction() {
        assert_eq!(
            extract_method_name("no method named `stream_tokens` found for struct `Session`"),
            Some("stream_tokens".to_string())
        );
        assert_eq!(
            extract_method_name("Property 'executeAsync' does not exist on type 'Worker'"),
            Some("executeAsync".to_string())
        );
        assert_eq!(
            extract_method_name("client.FetchBatch undefined (type Client has no field or method FetchBatch)"),
            Some("FetchBatch".to_string())
        );
        assert_eq!(
            extract_method_name("'Manager' object has no attribute 'dispatch_event'"),
            Some("dispatch_event".to_string())
        );
        assert_eq!(
            extract_method_name("no member named 'compute_digest' in 'HashBuilder'"),
            Some("compute_digest".to_string())
        );
    }

    #[test]
    fn hallucination_classification_rules() {
        let borrow_diag = DocDiagnostic {
            severity: "error".to_string(),
            code: Some("E0502".to_string()),
            message: "cannot borrow `cache` as mutable because it is also borrowed as immutable".to_string(),
            line: 42,
            col: 10,
            source: Some("rustc".to_string()),
            note: None,
            end: None,
        };
        let intercept = classify_hallucination(&borrow_diag).expect("classified as hallucination");
        assert_eq!(intercept.kind, HallucinationKind::BorrowCheckerError);
        assert!(intercept.suggestion.unwrap().contains("borrow"));

        let method_diag = DocDiagnostic {
            severity: "error".to_string(),
            code: Some("unresolved-method".to_string()),
            message: "no method named `nonexistent_api` found for struct `Client`".to_string(),
            line: 15,
            col: 8,
            source: Some("rust-analyzer".to_string()),
            note: None,
            end: None,
        };
        let intercept = classify_hallucination(&method_diag).expect("classified as hallucination");
        assert_eq!(intercept.kind, HallucinationKind::InvalidMethodInvocation);
        assert_eq!(intercept.symbol_or_target.as_deref(), Some("nonexistent_api"));

        let type_diag = DocDiagnostic {
            severity: "error".to_string(),
            code: Some("E0308".to_string()),
            message: "mismatched types: expected `u64`, found `&str`".to_string(),
            line: 25,
            col: 12,
            source: Some("rustc".to_string()),
            note: None,
            end: None,
        };
        let intercept = classify_hallucination(&type_diag).expect("classified as hallucination");
        assert_eq!(intercept.kind, HallucinationKind::IncorrectArgumentType);

        let syntax_diag = DocDiagnostic {
            severity: "error".to_string(),
            code: Some("syntax-error".to_string()),
            message: "expected `;`, found `}`".to_string(),
            line: 30,
            col: 1,
            source: Some("rust-analyzer".to_string()),
            note: None,
            end: None,
        };
        let intercept = classify_hallucination(&syntax_diag).expect("classified as hallucination");
        assert_eq!(intercept.kind, HallucinationKind::SyntaxError);
    }

    #[test]
    fn diagnostics_report_and_stream_result_render() {
        let intercept = HallucinationInterception {
            kind: HallucinationKind::InvalidMethodInvocation,
            symbol_or_target: Some("hallucinated_fn".to_string()),
            message: "method `hallucinated_fn` does not exist".to_string(),
            line: 12,
            col: 5,
            suggestion: Some("Check symbol declarations via code_definition".to_string()),
        };
        let report = DiagnosticsReport {
            file: "src/engine.rs".to_string(),
            errors: 1,
            warnings: 0,
            items: vec![DocDiagnostic {
                severity: "error".to_string(),
                code: Some("unresolved-method".to_string()),
                message: "method `hallucinated_fn` does not exist".to_string(),
                line: 12,
                col: 5,
                source: Some("rust-analyzer".to_string()),
                note: None,
                end: None,
            }],
            preexisting: vec![],
            in_derive: vec![],
            auto_trait: vec![],
            hallucinations: vec![intercept.clone()],
        };

        let rendered = report.render();
        assert!(rendered.contains("=== Intercepted Hallucinations (Phase 7.7) ==="));
        assert!(rendered.contains("[INTERCEPT] InvalidMethodInvocation:"));
        assert!(rendered.contains("hallucinated_fn"));
        assert!(rendered.contains("Check symbol declarations"));

        let stream_res = StreamValidationResult {
            completed_chunks: 3,
            total_chunks: 5,
            intercepted: true,
            interception: Some(intercept),
            intercept_chunk_index: Some(2),
            final_report: Some(report),
            summary: "intercepted InvalidMethodInvocation at chunk 3".to_string(),
        };

        let stream_rendered = stream_res.render();
        assert!(stream_rendered.contains("Streamed validation:"));
        assert!(stream_rendered.contains("[INTERCEPT at chunk 3]:"));
        assert!(stream_rendered.contains("--> Check symbol declarations"));

        // Verify JSON serialization roundtrip
        let serialized = serde_json::to_string(&stream_res).unwrap();
        let deserialized: StreamValidationResult = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized.completed_chunks, 3);
        assert_eq!(deserialized.intercepted, true);
    }
}
