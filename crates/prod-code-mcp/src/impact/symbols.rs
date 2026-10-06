/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;

use super::incoming::unreadable;

/// Known agent and editor scratch or cache directories whose untracked contents are not project source (#760).
const SCRATCH_DIRS: &[&str] = &[
    ".prod", ".scratch", ".tmp", ".cache", ".vscode", ".idea", ".claude", ".cursor",
];

/// Whether an untracked path lies in a known scratch or cache directory (e.g. `.prod/`, `.scratch/`, `.tmp/`) (#760).
pub fn is_scratch_path(path: &str) -> bool {
    Path::new(path).components().any(|c| match c {
        std::path::Component::Normal(s) => {
            let name = s.to_string_lossy();
            SCRATCH_DIRS.iter().any(|d| name == *d)
        }
        _ => false,
    })
}

pub(crate) fn is_source_file(path: &str) -> bool {
    matches!(
        Path::new(path).extension().and_then(|e| e.to_str()),
        Some(
            "rs" | "go"
                | "py"
                | "pyi"
                | "ts"
                | "tsx"
                | "mts"
                | "cts"
                | "js"
                | "jsx"
                | "mjs"
                | "cjs"
                | "c"
                | "cc"
                | "cpp"
                | "cxx"
                | "h"
                | "hpp"
                | "hh"
                | "swift"
                | "m"
                | "mm"
                | "java"
                | "kt"
                | "kts"
                | "cs"
                | "scala"
                | "sc"
                | "zig"
        )
    )
}

/// The 1-based line or column an LSP position's 0-based `field` gives, when it is a number
/// that fits: a negative, fractional or oversized one is unreadable, not line 1.
pub fn one_based(position: &serde_json::Value, field: &str) -> Option<u32> {
    u32::try_from(position.get(field)?.as_u64()?)
        .ok()?
        .checked_add(1)
}

pub(crate) fn symbol_range(sym: &serde_json::Value) -> Option<(u32, u32, u32, u32)> {
    let range = sym
        .get("range")
        .or_else(|| sym.get("location").and_then(|l| l.get("range")))?;
    let sel = sym.get("selectionRange").unwrap_or(range);
    let start = one_based(range.get("start")?, "line")?;
    let end = one_based(range.get("end")?, "line")?;
    let sl = one_based(sel.get("start")?, "line")?;
    let sc = one_based(sel.get("start")?, "character")?;
    (start <= end).then_some((start, end, sl, sc))
}

pub(crate) fn is_callable_symbol(kind: u64, sym: &serde_json::Value, text: Option<&str>) -> bool {
    if matches!(kind, 6 | 9 | 12) {
        return true;
    }
    if matches!(kind, 7 | 8 | 13 | 14) {
        if let Some(detail) = sym.get("detail").and_then(|d| d.as_str())
            && (detail.contains("=>") || detail.contains("function") || detail.contains('('))
        {
            return true;
        }
        if let Some(text) = text
            && let Some((_, _, sl, _)) = symbol_range(sym)
            && sl > 0
        {
            let lines: Vec<&str> = text.lines().collect();
            let idx = (sl - 1) as usize;
            for line in lines.iter().skip(idx).take(3) {
                let trimmed = line.trim();
                if trimmed.contains("=>")
                    || trimmed.contains("function")
                    || trimmed.contains("async ")
                {
                    return true;
                }
            }
        }
    }
    false
}

/// Functions and methods (LSP kinds 6, 9, 12, and callable properties/variables) in a document,
/// flattened with their ranges. An entry that is not a symbol (no name or kind, children that
/// are not a list, a function without a readable range) is an error: skipped, it would hide a
/// changed function.
pub(crate) fn collect_functions(
    symbols: &[serde_json::Value],
    text: Option<&str>,
    out: &mut Vec<(String, u32, u32, u32, u32)>,
) -> std::result::Result<(), String> {
    for sym in symbols {
        let malformed = || unreadable("textDocument/documentSymbol", sym);
        let (Some(name), Some(kind)) = (
            sym.get("name").and_then(|n| n.as_str()),
            sym.get("kind").and_then(|k| k.as_u64()),
        ) else {
            return Err(malformed());
        };
        if name.is_empty() || !(1..=26).contains(&kind) {
            return Err(malformed());
        }
        if is_callable_symbol(kind, sym, text) && !name.is_empty() {
            let (start, end, sl, sc) = symbol_range(sym).ok_or_else(malformed)?;
            out.push((name.to_string(), start, end, sl, sc));
        }
        match sym.get("children") {
            None | Some(serde_json::Value::Null) => {}
            Some(serde_json::Value::Array(children)) => collect_functions(children, text, out)?,
            Some(_) => return Err(malformed()),
        }
    }
    Ok(())
}
