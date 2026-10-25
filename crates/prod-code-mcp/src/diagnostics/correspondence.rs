/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Conservative occurrence correspondence: a line must have the same partner in every
//! longest common subsequence. Picking just one diff alignment can hide a new occurrence.

use std::path::Path;

use crate::lang::language_id_for_path;

/// Return the earliest and latest embeddings of a complete subsequence, if it is one.
fn embeddings(needle: &[&str], haystack: &[&str]) -> Option<(Vec<usize>, Vec<usize>)> {
    let mut earliest = Vec::with_capacity(needle.len());
    let mut cursor = 0;
    for line in needle {
        cursor += haystack[cursor..]
            .iter()
            .position(|candidate| candidate == line)?;
        earliest.push(cursor);
        cursor += 1;
    }
    let mut latest = Vec::with_capacity(needle.len());
    cursor = haystack.len();
    for line in needle.iter().rev() {
        cursor = haystack[..cursor]
            .iter()
            .rposition(|candidate| candidate == line)?;
        latest.push(cursor);
    }
    latest.reverse();
    Some((earliest, latest))
}

/// Without a language's structural context, retain all code lines in order. Arbitrary
/// inserted/deleted code can change ownership even when one snapshot embeds in the other.
/// Permit blank lines and inert whole-line comments for known slash-comment languages
/// and Python. Hash comments use the same restricted alphabet as slash comments.
/// The comment alphabet excludes literal/interpolation/comment delimiters: even inside
/// a multiline literal these lines cannot open or close a different lexical context.
pub(super) fn whole_snapshot_embedding(before: &str, after: &str, file: &str) -> bool {
    // A new blank/comment line can terminate an existing escaped-newline directive.
    // Without a language lexer, do not certify edits across line continuations.
    if before != after
        && [before, after]
            .iter()
            .any(|text| text.contains("\\\n") || text.contains("\\\r\n"))
    {
        return false;
    }
    let comment_prefix = match language_id_for_path(Path::new(file)) {
        "typescript" | "typescriptreact" | "javascript" | "javascriptreact" | "go" | "c"
        | "cpp" | "java" | "kotlin" | "csharp" | "swift" => Some("//"),
        "python" => Some("#"),
        _ => None,
    };
    let significant = |line: &&str| {
        let line = line.trim();
        !line.is_empty()
            && !comment_prefix.is_some_and(|prefix| {
                line.strip_prefix(prefix).is_some_and(|comment| {
                    comment
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b" \t.,!?_-".contains(&byte))
                })
            })
    };
    before
        .lines()
        .filter(significant)
        .eq(after.lines().filter(significant))
}

/// A one-based old line for each new line, only when its correspondence is unambiguous.
/// Full-line equality is necessary but is never by itself the occurrence identity.
pub(super) fn unchanged_lines(before: &str, after: &str) -> Vec<Option<u32>> {
    let old: Vec<_> = before.lines().collect();
    let new: Vec<_> = after.lines().collect();
    let mut mapped = vec![None; new.len()];
    // Insertions/deletions (including no-op snapshots) admit linear-time proof. Earliest
    // and latest embeddings agree exactly on the forced pairs, even with repeated lines.
    if let Some((first, last)) = embeddings(&old, &new) {
        for (i, (first, last)) in first.into_iter().zip(last).enumerate() {
            if first == last {
                mapped[first] = Some(i as u32 + 1);
            }
        }
        return mapped;
    }
    if let Some((first, last)) = embeddings(&new, &old) {
        for (j, (first, last)) in first.into_iter().zip(last).enumerate() {
            if first == last {
                mapped[j] = Some(first as u32 + 1);
            }
        }
        return mapped;
    }

    let width = new.len() + 1;
    // Bound interactive memory/work for mixed edits. Insufficient correspondence evidence
    // leaves diagnostics blocking; it must never turn an expensive comparison into clean.
    let Some(cells) = (old.len() + 1)
        .checked_mul(width)
        .filter(|n| *n <= 2_000_000)
    else {
        return mapped;
    };
    let mut prefix = vec![0u32; cells];
    let mut suffix = vec![0u32; cells];
    for i in 0..old.len() {
        for j in 0..new.len() {
            prefix[(i + 1) * width + j + 1] = if old[i] == new[j] {
                prefix[i * width + j] + 1
            } else {
                prefix[i * width + j + 1].max(prefix[(i + 1) * width + j])
            };
        }
    }
    for i in (0..old.len()).rev() {
        for j in (0..new.len()).rev() {
            suffix[i * width + j] = if old[i] == new[j] {
                suffix[(i + 1) * width + j + 1] + 1
            } else {
                suffix[(i + 1) * width + j].max(suffix[i * width + j + 1])
            };
        }
    }
    let longest = suffix[0];
    for j in 0..new.len() {
        let mut partner = None;
        let mut ambiguous = false;
        for i in 0..=old.len() {
            // An optimal path can skip this new line entirely.
            if prefix[i * width + j] + suffix[i * width + j + 1] == longest {
                ambiguous = true;
                break;
            }
            if i < old.len()
                && old[i] == new[j]
                && prefix[i * width + j] + 1 + suffix[(i + 1) * width + j + 1] == longest
            {
                if partner.is_some() {
                    ambiguous = true;
                    break;
                }
                partner = Some(i as u32 + 1);
            }
        }
        if !ambiguous {
            mapped[j] = partner;
        }
    }
    mapped
}

/// Map an entire diagnostic range, refusing changed interiors and unknown coordinates.
pub(super) fn old_range(
    lines: &[Option<u32>],
    line: u32,
    end: Option<(u32, u32)>,
) -> Option<(u32, Option<(u32, u32)>)> {
    let old = *lines.get(line.checked_sub(1)? as usize)?.as_ref()?;
    let mapped_end = if let Some((end_line, col)) = end {
        if end_line < line {
            return None;
        }
        for current in line..=end_line {
            if lines.get(current as usize - 1).copied().flatten() != Some(old + current - line) {
                return None;
            }
        }
        Some((old + end_line - line, col))
    } else {
        None
    };
    Some((old, mapped_end))
}
