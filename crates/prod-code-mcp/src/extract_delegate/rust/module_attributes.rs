/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::fixture::builder::lexer::Source;
use anyhow::{Context, Result};

#[derive(Clone, Default, PartialEq, Eq)]
pub(super) struct Attributes {
    pub path: Option<String>,
    pub disabled: bool,
    pub uncertain: bool,
}

// Keep both effects of target-dependent cfg_attr. Neither branch is a definite
// module identity, but its possible path remains available to an occurrence.
pub(super) fn apply(
    source: &Source<'_>,
    start: usize,
    end: usize,
    states: Vec<Attributes>,
) -> Result<Vec<Attributes>> {
    let mut result = Vec::new();
    for mut state in states {
        if source.is(start, "path") && source.is(start + 1, "=") {
            state.path = Some(
                serde_json::from_str(source.t(start + 2))
                    .context("unsupported module path literal; nothing was planned")?,
            );
        } else if source.is(start, "cfg") && source.is(start + 1, "(") {
            let value = constant_cfg(source, start + 2, end - 1);
            state.disabled |= value == Some(false);
            state.uncertain |= value.is_none();
        } else if source.is(start, "cfg_attr") && source.is(start + 1, "(") {
            let close = source.close_of(start + 1).context("unclosed cfg_attr")?;
            anyhow::ensure!(
                close + 1 == end,
                "unsupported cfg_attr; nothing was planned"
            );
            let parts = parts(source, start + 2, close)?;
            let (predicate_start, predicate_end) =
                *parts.first().context("missing cfg_attr predicate")?;
            let predicate = constant_cfg(source, predicate_start, predicate_end);
            if predicate != Some(false) {
                let mut nested = vec![state.clone()];
                for &(start, end) in &parts[1..] {
                    nested = apply(source, start, end, nested)?;
                }
                if predicate == Some(true) {
                    result.extend(nested);
                    continue;
                }
                // A target-dependent attribute with no path/presence effect
                // (e.g. allow, or a constant-false nested predicate) is harmless.
                if nested.iter().any(|s| s != &state) {
                    state.uncertain = true;
                    for mut alternative in nested {
                        alternative.uncertain = true;
                        if !result.contains(&alternative) {
                            result.push(alternative);
                        }
                    }
                }
            }
        }
        if !result.contains(&state) {
            result.push(state);
        }
    }
    Ok(result)
}

fn parts(source: &Source<'_>, start: usize, end: usize) -> Result<Vec<(usize, usize)>> {
    let mut result = Vec::new();
    let mut item = start;
    let mut at = start;
    while at < end {
        if source.is(at, ",") {
            if item < at {
                result.push((item, at));
            }
            item = at + 1;
        } else if ["(", "[", "{"].iter().any(|s| source.is(at, s)) {
            at = source
                .close_of(at)
                .context("unclosed module attribute argument")?;
        }
        at += 1;
    }
    if item < end {
        result.push((item, end));
    }
    Ok(result)
}

// Only fold cfg predicates independent of target/features. Unknown leaves
// remain unknown; this is not a substitute for Cargo's active cfg state.
pub(super) fn constant_cfg(source: &Source<'_>, start: usize, end: usize) -> Option<bool> {
    if start + 2 >= end || !source.is(start + 1, "(") {
        return None;
    }
    let close = source.close_of(start + 1)?;
    if close + 1 != end {
        return None;
    }
    let values: Vec<_> = parts(source, start + 2, close)
        .ok()?
        .into_iter()
        .map(|(start, end)| constant_cfg(source, start, end))
        .collect();
    match source.t(start) {
        "not" if values.len() == 1 => values[0].map(|v| !v),
        "any" if values.contains(&Some(true)) => Some(true),
        "any" if values.iter().all(|v| *v == Some(false)) => Some(false),
        "all" if values.contains(&Some(false)) => Some(false),
        "all" if values.iter().all(|v| *v == Some(true)) => Some(true),
        _ => None,
    }
}
