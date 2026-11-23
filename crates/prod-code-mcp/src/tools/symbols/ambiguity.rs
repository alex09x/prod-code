/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::SymbolHit;
use std::path::Path;

/// A bounded, ranked preview, including signatures where source is available (#1120).
pub(crate) fn ambiguous_candidates_error(
    root: &Path,
    symbol: &str,
    ties: &[SymbolHit],
) -> anyhow::Error {
    let mut msg = format!(
        "`{symbol}` is ambiguous ({} candidates); qualify it (Type::name) or pass `path`:\n",
        ties.len()
    );
    for (index, hit) in ties.iter().take(10).enumerate() {
        let snippet = std::fs::read_to_string(&hit.path).ok().and_then(|text| {
            text.lines()
                .nth(hit.line.saturating_sub(1) as usize)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.chars().take(160).collect::<String>())
        });
        msg.push_str(&format!("  {}. {}", index + 1, hit.render(root)));
        if let Some(snippet) = snippet {
            msg.push_str(&format!(" — `{snippet}`"));
        }
        msg.push('\n');
    }
    if ties.len() > 10 {
        msg.push_str(&format!(
            "  ({} further candidates omitted)\n",
            ties.len() - 10
        ));
    }
    anyhow::anyhow!("{}", msg.trim_end())
}

/// Whether a `path` given with a symbol names one file (`src/a.rs`), not a directory or a
/// project that only selects where to search.
pub(crate) fn names_a_file(hint: &Path) -> bool {
    hint.extension().is_some()
}

/// Whether `hit` is in the file `hint` names, given relative to `root` or absolute.
pub(crate) fn in_hinted_file(root: &Path, hint: &Path, hit: &SymbolHit) -> bool {
    let relative: std::path::PathBuf = hint
        .components()
        .filter(|c| !matches!(c, std::path::Component::CurDir))
        .collect();
    if hint.is_absolute() {
        hit.path == relative
    } else {
        hit.path == root.join(&relative) || hit.path.ends_with(&relative)
    }
}

/// The answer when `hint` names a file that has no candidate: where the candidates are.
pub(crate) fn not_in_hint_message(
    root: &Path,
    symbol: &str,
    hint: &Path,
    elsewhere: &[SymbolHit],
) -> String {
    let mut msg = format!(
        "no `{symbol}` in {}, the file given to disambiguate",
        hint.strip_prefix(root).unwrap_or(hint).display()
    );
    if elsewhere.is_empty() {
        return msg;
    }
    msg.push_str(
        "; found only elsewhere (drop `path` to accept one, or check that the file is current):\n",
    );
    for hit in elsewhere.iter().take(5) {
        msg.push_str(&format!("  - {}\n", hit.render(root)));
    }
    if elsewhere.len() > 5 {
        msg.push_str(&format!("  ({} more)\n", elsewhere.len() - 5));
    }
    msg.trim_end().to_string()
}
