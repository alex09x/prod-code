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
