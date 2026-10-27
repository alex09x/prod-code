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

use crate::diagnostics::correspondence::{
    in_line_token_edit, old_range, same_lines, unchanged_lines, whole_snapshot_embedding,
};
pub use crate::diagnostics::ident::mentions_identifier;
use crate::diagnostics::parse::{INVALID_DIAGNOSTICS, classify_hallucination};
use crate::diagnostics::scope::RustContexts;
use crate::diagnostics::types::{DiagnosticsReport, DocDiagnostic};

/// Diagnostic content identity. This alone does not establish occurrence correspondence.
pub fn identity(d: &DocDiagnostic, text: &str) -> (String, Option<String>, String, String) {
    let line = text
        .lines()
        .nth(d.line.saturating_sub(1) as usize)
        .unwrap_or("")
        .trim()
        .to_string();
    (d.severity.clone(), d.code.clone(), d.message.clone(), line)
}

pub fn refresh_hallucinations(report: &mut DiagnosticsReport) {
    report.hallucinations = report
        .items
        .iter()
        .filter(|diagnostic| diagnostic.severity == "error")
        .filter_map(classify_hallucination)
        .collect();
}

fn unchecked(d: &DocDiagnostic) -> bool {
    matches!(d.code.as_deref(), Some(code) if code == prod_code_protocol::ANALYZER_PANIC_CODE
        || code == UNLINKED_FILE || code == INVALID_DIAGNOSTICS)
}

/// Moves from `report.items` to `report.preexisting` every diagnostic that `before` — the same
/// file's diagnostics against `before_text`, the text on disk — already had. Each diagnostic
/// before the edit accounts for at most one after it, at a proven unchanged occurrence.
/// Ambiguous line correspondence and changed ranges remain in the blocking report.
pub fn set_aside_preexisting(
    report: &mut DiagnosticsReport,
    text: &str,
    before: &DiagnosticsReport,
    before_text: &str,
) {
    // Rust has structural evidence below. Other languages require retained code context;
    // even insertion-only alignments can transfer a line into a newly introduced scope.
    let lines = if report.file.ends_with(".rs")
        || whole_snapshot_embedding(before_text, text, &report.file)
    {
        unchanged_lines(before_text, text)
    } else if in_line_token_edit(before_text, text) {
        // Edits inside lines that keep every line's shape, as a rename's: an untouched line is
        // the same line, so its unchanged error is the file's own (#1491).
        same_lines(before_text, text)
    } else {
        Vec::new()
    };
    let rust_contexts = report
        .file
        .ends_with(".rs")
        .then(|| (RustContexts::new(before_text), RustContexts::new(text)));
    let mut old = HashMap::new();
    for d in &before.items {
        *old.entry((
            identity(d, before_text),
            d.line,
            d.col,
            d.end,
            d.source.clone(),
        ))
        .or_insert(0usize) += 1;
    }
    let items = std::mem::take(&mut report.items);
    for d in items {
        // A file the analyzer panicked on, or that no crate includes, was not checked; that it
        // was not checked before the edit either does not make it checked now (#94, #467).
        if unchecked(&d) {
            report.items.push(d);
            continue;
        }
        let range = old_range(&lines, d.line, d.end);
        let count = range.and_then(|(line, end)| {
            if let Some((before, after)) = &rust_contexts {
                match (before, after) {
                    (Some(before), Some(after))
                        if after.corresponds(before, &lines, (d.line, d.col), (line, d.col)) => {}
                    _ => return None,
                }
            }
            old.get_mut(&(identity(&d, text), line, d.col, end, d.source.clone()))
        });
        match count {
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

/// Explain possible derive/auto-trait analyzer limitations without discarding error authority:
/// attribute/message shape is not proof that rustc accepts the code (#1286).
/// The existing detached/test prelude-macro policy is unchanged (#950).
pub fn set_aside_derive_expansions(report: &mut DiagnosticsReport, text: &str) {
    let rust = report.file.ends_with(".rs");
    let disabled_prelude = text.contains("no_implicit_prelude") || text.contains("no_std");
    let is_test = report.file.contains("test")
        || report.file.ends_with("_test.rs")
        || report.file.ends_with("_tests.rs")
        || report.file.contains("/tests/")
        || text.contains("#[cfg(test)]")
        || text.contains("#[test]")
        || text.contains("mod tests");
    let prelude_limitation = rust && is_test && !disabled_prelude;
    let items = std::mem::take(&mut report.items);
    for mut d in items {
        if unchecked(&d) {
            report.items.push(d);
            continue;
        }
        let line = text
            .lines()
            .nth(d.line.saturating_sub(1) as usize)
            .unwrap_or("");
        let limitation = if rust
            && d.code.as_deref() == Some("E0282")
            && line.trim_start().starts_with("#[derive(")
        {
            Some(
                "A derive expansion can expose an analyzer inference limitation, but the attribute is not proof: custom derives can produce genuine E0282 errors. This diagnostic remains counted; an explicit compiler check can distinguish them.",
            )
        } else if rust && d.code.as_deref() == Some("E0277") && is_auto_trait_bound(&d.message) {
            Some(
                "rust-analyzer can fail to prove Send, Sync or Unpin, but the message is not proof of an analyzer limitation: genuine Rust E0277 errors use the same wording. This diagnostic remains counted; an explicit compiler check can distinguish them.",
            )
        } else {
            None
        };
        if let Some(limitation) = limitation {
            if !d
                .note
                .as_deref()
                .is_some_and(|note| note.contains(limitation))
            {
                d.note = Some(match d.note.take() {
                    Some(note) => format!("{note}\n{limitation}"),
                    None => limitation.into(),
                });
            }
            report.items.push(d);
        } else if prelude_limitation && is_unresolved_prelude_macro(&d.message) {
            report.auto_trait.push(d);
        } else {
            report.items.push(d);
        }
    }
    let preexisting = std::mem::take(&mut report.preexisting);
    for d in preexisting {
        if prelude_limitation && is_unresolved_prelude_macro(&d.message) {
            report.auto_trait.push(d);
        } else {
            report.preexisting.push(d);
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
pub fn is_auto_trait_bound(message: &str) -> bool {
    let first = message.lines().next().unwrap_or("");
    [": Send`", ": Sync`", ": Unpin`"]
        .iter()
        .any(|bound| first.contains(bound))
        || first.contains("cannot be sent between threads safely")
        || first.contains("cannot be shared between threads safely")
        || first.contains("cannot be unpinned")
}

/// Whether a diagnostic message is about an unresolved Rust prelude macro (`assert_eq!`, `vec!`,
/// `format!`, etc.) emitted due to analyzer limitation in detached/test module validation (#950).
pub fn is_unresolved_prelude_macro(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    if !lower.contains("unresolved macro") && !lower.contains("cannot find macro") {
        return false;
    }
    const PRELUDE_MACROS: &[&str] = &[
        "assert",
        "assert_eq",
        "assert_ne",
        "debug_assert",
        "debug_assert_eq",
        "debug_assert_ne",
        "vec",
        "format",
        "println",
        "eprintln",
        "print",
        "eprint",
        "panic",
        "todo",
        "unimplemented",
        "unreachable",
        "matches",
        "cfg",
        "env",
        "option_env",
        "concat",
        "stringify",
        "include",
        "include_str",
        "include_bytes",
        "write",
        "writeln",
        "dbg",
    ];
    PRELUDE_MACROS.iter().any(|mac| {
        let needle = format!("{mac}!");
        let needle_tick = format!("`{mac}!`");
        let needle_bare = format!("`{mac}`");
        let needle_space = format!(" {mac} ");
        message.contains(&needle)
            || message.contains(&needle_tick)
            || message.contains(&needle_bare)
            || message.contains(&needle_space)
            || message.ends_with(&format!(" {mac}"))
    })
}

/// rust-analyzer's code for a file that no crate includes: it offers no semantic service there,
/// so it reports no type error, no unresolved name, nothing but this hint (#467).
pub const UNLINKED_FILE: &str = "unlinked-file";

/// What a validation says of an `unlinked-file`: why it counts, that it is not a type error, and
/// what would check the file.
pub const UNLINKED_NOTE: &str = "rust-analyzer includes this file in no crate, so it was not \
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
pub fn refuse_unchecked(report: &mut DiagnosticsReport) {
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
pub const SYMBOL_KIND_METHOD: u64 = 6;
/// LSP `SymbolKind::Constructor`: 9
pub const SYMBOL_KIND_CONSTRUCTOR: u64 = 9;
/// LSP `SymbolKind::Function`: 12
pub const SYMBOL_KIND_FUNCTION: u64 = 12;
/// LSP `SymbolKind::Variable`: rust-analyzer lists a function's `let` bindings under it.
pub const SYMBOL_KIND_VARIABLE: u64 = 13;

/// Every symbol name in a `textDocument/documentSymbol` result (flat or hierarchical) that
/// another file could refer to. A local variable is listed too, and it is not one of them: a
/// `let edit` removed from one function is not what another file's `let edit` names (#136).
/// Declarations and returned properties nested inside a function, method, or constructor
/// are local execution details that external files cannot refer to (#818).
pub fn symbol_names(result: &serde_json::Value) -> BTreeSet<String> {
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
