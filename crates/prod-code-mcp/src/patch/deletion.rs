/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{Hunk, diff_line};
use anyhow::{Context, Result, ensure};
use std::{iter::Peekable, path::Path, str::SplitInclusive};

const NO_NEWLINE: &str = "\\ No newline at end of file";

/// A deletion has no new lines: consume its complete, count-bounded old ranges.
/// In particular, a removed line beginning `-- ` is data while its range is open.
pub(super) fn parse_hunks(
    lines: &mut Peekable<SplitInclusive<'_, char>>,
) -> Result<(Vec<Hunk>, bool)> {
    let mut hunks = Vec::new();
    let mut consumed = 0usize;
    let mut without_newline = false;
    while let Some(&raw) = lines.peek() {
        let line = diff_line(raw);
        if line.starts_with("--- ") || line.starts_with("diff --git ") {
            break;
        }
        lines.next();
        if line.is_empty() {
            continue;
        }
        ensure!(
            !without_newline,
            "deletion data follows its final newline marker"
        );
        let (old, new) = line
            .strip_prefix("@@ -")
            .and_then(|s| s.split_once(" +"))
            .context("invalid deletion hunk header")?;
        let (new, _) = new
            .split_once(" @@")
            .context("invalid deletion hunk header")?;
        ensure!(new == "0,0", "deletion hunk must have an empty new range");
        let (start, count) = old.split_once(',').unwrap_or((old, "1"));
        let start: usize = start.parse().context("invalid deletion hunk start")?;
        let count: usize = count.parse().context("invalid deletion hunk count")?;
        ensure!(
            if count == 0 {
                start == 0 && hunks.is_empty()
            } else {
                start == consumed + 1
            },
            "deletion hunks must cover the complete old file in order"
        );
        let mut body = Vec::new();
        for _ in 0..count {
            let removed = lines
                .next()
                .and_then(|raw| raw.strip_suffix('\n').unwrap_or(raw).strip_prefix('-'))
                .context("truncated or malformed deletion hunk")?;
            body.push(('-', removed.to_string()));
        }
        if lines.peek().map(|raw| diff_line(raw)) == Some(NO_NEWLINE) {
            ensure!(count > 0, "newline marker without a removed line");
            lines.next();
            without_newline = true;
        }
        consumed += count;
        hunks.push(Hunk {
            old_start: start,
            lines: body,
            new_ends_without_newline: false,
        });
    }
    ensure!(
        hunks.len() <= 1 || hunks.iter().all(|h| !h.lines.is_empty()),
        "empty deletion hunk mixed with nonempty hunks"
    );
    Ok((hunks, without_newline))
}

/// Validate against the live file only. A Git preimage cannot authorize its removal.
pub(super) fn validate(
    path: &Path,
    shown: &str,
    hunks: &[Hunk],
    without_newline: bool,
) -> Result<()> {
    let current = std::fs::read_to_string(path)
        .with_context(|| format!("the diff deletes {shown}, which cannot be read"))?;
    // The parser requires ordered, complete removal ranges. Reconstruct their exact
    // preimage: line-based hunk matching would normalize CRLF and permit stale content.
    let mut preimage = String::new();
    for hunk in hunks {
        for (_, removed) in &hunk.lines {
            preimage.push_str(removed);
            preimage.push('\n');
        }
    }
    if without_newline {
        preimage.pop();
    }
    ensure!(
        preimage == current,
        "deletion of {shown} does not match the complete live file contents"
    );
    Ok(())
}
