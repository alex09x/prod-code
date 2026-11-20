/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Errors a move carries, unchanged, out of the file it cuts from (#2038).
//!
//! A move cuts one block of lines out of a file and writes it into another file of the same
//! batch. When the analyzer already reported an error before the move on a line the move left in
//! place, or on a line it carried along, it reports the same error on the same text after it;
//! that error is not the move's. The per-file comparison cannot see this outside Rust: the
//! target has no "before", and a cut leaves no line correspondence it trusts. A Swift script
//! compiled through an explicit `swiftc` file list, outside every SwiftPM target, is checked by
//! the analyzer alone, so each peer declaration it uses is already "cannot find 'Driver' in
//! scope" on disk, and every one of those lines was reported as the move's own error.
//!
//! The authority is taken from the move's shape, not from line text alone: a file counts as cut
//! only when its proposed text is its text on disk with one contiguous block of lines removed,
//! and that block appears, line for line, in another file of the batch. Any other edit — an
//! insertion, a line moved within one file — gets nothing here and stays with the per-file
//! comparison, which refuses to carry an error into another scope.

use std::collections::HashMap;

use crate::diagnostics::filter::{identity, refresh_hallucinations, unchecked};
use crate::diagnostics::types::{DiagnosticsReport, DocDiagnostic};

/// Content, trimmed line text, column from the first non-blank character, and source.
type Key = (
    (String, Option<String>, String, String),
    u32,
    Option<String>,
);

fn key(d: &DocDiagnostic, text: &str) -> Key {
    let line = text
        .lines()
        .nth(d.line.saturating_sub(1) as usize)
        .unwrap_or("");
    let indent = line.chars().take_while(|c| c.is_whitespace()).count() as u32;
    (
        identity(d, text),
        d.col.saturating_sub(indent),
        d.source.clone(),
    )
}

/// The 0-based line range `[start, end)` of `before` whose removal gives `after`, when the edit
/// removed exactly one non-empty block and changed nothing else.
fn cut_block(before: &str, after: &str) -> Option<(usize, usize)> {
    let b: Vec<&str> = before.lines().collect();
    let a: Vec<&str> = after.lines().collect();
    if a.len() >= b.len() {
        return None;
    }
    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    (prefix + suffix == a.len()).then_some((prefix, b.len() - suffix))
}

/// The 1-based line ranges `[first, last]` of `text` where the non-blank lines of `block`,
/// trimmed, appear consecutively.
fn block_places(text: &str, block: &[&str]) -> Vec<(u32, u32)> {
    let wanted: Vec<&str> = block
        .iter()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();
    if wanted.is_empty() {
        return Vec::new();
    }
    let lines: Vec<(usize, &str)> = text
        .lines()
        .map(str::trim)
        .enumerate()
        .filter(|(_, l)| !l.is_empty())
        .collect();
    if lines.len() < wanted.len() {
        return Vec::new();
    }
    (0..=lines.len() - wanted.len())
        .filter(|&at| {
            lines[at..at + wanted.len()]
                .iter()
                .zip(&wanted)
                .all(|((_, l), w)| l == w)
        })
        .map(|at| {
            (
                lines[at].0 as u32 + 1,
                lines[at + wanted.len() - 1].0 as u32 + 1,
            )
        })
        .collect()
}

/// Where in the other files of the batch the move put `block`: the places it has in the
/// proposed text and did not have on disk, so a copy that was already there carries nothing.
fn landings(
    file: &str,
    block: &[&str],
    sources: &HashMap<String, String>,
    baselines: &HashMap<String, (DiagnosticsReport, String)>,
) -> HashMap<String, Vec<(u32, u32)>> {
    let mut placed = HashMap::new();
    for (other, text) in sources {
        if other == file {
            continue;
        }
        let after = block_places(text, block);
        let before = baselines
            .get(other)
            .map_or(0, |(_, before)| block_places(before, block).len());
        if after.len() > before {
            placed.insert(other.clone(), after);
        }
    }
    placed
}

fn recount(report: &mut DiagnosticsReport) {
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

/// Sets aside, in the non-Rust files of a move, the errors the analyzer already reported before
/// it: in a cut file, an error on a line the move left in place, at the same line after the cut;
/// in any other file of the batch, an error on a line the move carried out of the cut block.
/// Each error before accounts for at most one after it. An unchecked file's marker is never set
/// aside (#94, #467), and Rust keeps its structural comparison alone (#1286).
///
/// `baselines` holds, per displayed file, the diagnostics and text on disk; `sources` the
/// proposed text of each file.
pub fn set_aside_carried(
    reports: &mut [DiagnosticsReport],
    sources: &HashMap<String, String>,
    baselines: &HashMap<String, (DiagnosticsReport, String)>,
) {
    // Errors the move left in place, per cut file and line after the cut; errors it carried.
    let mut kept: HashMap<(String, u32), Vec<Key>> = HashMap::new();
    let mut carried: HashMap<Key, usize> = HashMap::new();
    // Per file, the lines a carried block landed on: a carried error is set aside only there.
    let mut landed_on: HashMap<String, Vec<(u32, u32)>> = HashMap::new();
    for (file, (before, before_text)) in baselines {
        if file.ends_with(".rs") {
            continue;
        }
        let Some(after) = sources.get(file) else {
            continue;
        };
        let Some((start, end)) = cut_block(before_text, after) else {
            continue;
        };
        let lines: Vec<&str> = before_text.lines().collect();
        let block = &lines[start..end];
        let placed = landings(file, block, sources, baselines);
        if placed.is_empty() {
            continue;
        }
        for (other, places) in placed {
            landed_on.entry(other).or_default().extend(places);
        }
        for d in before
            .items
            .iter()
            .filter(|d| d.severity == "error" && !unchecked(d))
        {
            let line = d.line.saturating_sub(1) as usize;
            if (start..end).contains(&line) {
                *carried.entry(key(d, before_text)).or_default() += 1;
            } else {
                let after_line = if line < start {
                    line
                } else {
                    line - (end - start)
                };
                kept.entry((file.clone(), after_line as u32 + 1))
                    .or_default()
                    .push(key(d, before_text));
            }
        }
    }
    if kept.is_empty() && carried.is_empty() {
        return;
    }
    for report in reports.iter_mut() {
        if report.file.ends_with(".rs") {
            continue;
        }
        let Some(text) = sources.get(&report.file) else {
            continue;
        };
        let cut_here = baselines
            .get(&report.file)
            .is_some_and(|(_, before)| cut_block(before, text).is_some());
        let items = std::mem::take(&mut report.items);
        let mut moved = false;
        for d in items {
            if d.severity != "error" || unchecked(&d) {
                report.items.push(d);
                continue;
            }
            let k = key(&d, text);
            let in_place = kept
                .get_mut(&(report.file.clone(), d.line))
                .and_then(|keys| keys.iter().position(|old| *old == k).map(|i| (keys, i)));
            if let Some((keys, i)) = in_place {
                keys.swap_remove(i);
                moved = true;
                report.preexisting.push(d);
                continue;
            }
            // A cut file only loses lines; what it still reports elsewhere is not carried, nor
            // is an error outside the lines the block landed on.
            let on_block = landed_on.get(&report.file).is_some_and(|places| {
                places
                    .iter()
                    .any(|(first, last)| (*first..=*last).contains(&d.line))
            });
            match (!cut_here && on_block)
                .then(|| carried.get_mut(&k))
                .flatten()
            {
                Some(n) if *n > 0 => {
                    *n -= 1;
                    moved = true;
                    report.preexisting.push(d);
                }
                _ => report.items.push(d),
            }
        }
        if moved {
            recount(report);
        }
    }
}

#[cfg(test)]
#[path = "carried_tests.rs"]
mod tests;
