/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result, ensure};
use std::path::PathBuf;

fn path(raw: &str) -> Result<String> {
    let raw = raw.split('\t').next().unwrap_or(raw).trim();
    ensure!(
        !raw.contains(['"', '\\']) && !raw.is_empty(),
        "unsupported diff path"
    );
    Ok(raw
        .strip_prefix("a/")
        .or_else(|| raw.strip_prefix("b/"))
        .unwrap_or(raw)
        .into())
}

/// Classify the entire input before reading source files. Counts distinguish literal
/// header-looking hunk content from operation metadata. Nothing outside this grammar
/// may silently disappear in the permissive text patch parser.
pub(super) fn paths(diff: &str) -> Result<Vec<(PathBuf, bool)>> {
    let mut lines = diff.lines().peekable();
    let mut git: Option<(String, String)> = None;
    let mut text_section = false;
    let mut hunk_seen = false;
    let mut files = Vec::new();
    while let Some(line) = lines.next() {
        if let Some(header) = line.strip_prefix("diff --git ") {
            ensure!(
                git.is_none() || text_section,
                "unsupported Git metadata-only section; no validation was performed"
            );
            ensure!(
                !text_section || hunk_seen,
                "unsupported text section without hunks"
            );
            let names: Vec<_> = header.split_whitespace().collect();
            ensure!(names.len() == 2, "unsupported Git path header");
            let old = path(names[0])?;
            let new = path(names[1])?;
            ensure!(
                old == new,
                "unsupported Git rename/copy; complete proposal not checked"
            );
            git = Some((old, new));
            text_section = false;
            hunk_seen = false;
        } else if let Some(old) = line.strip_prefix("--- ") {
            ensure!(
                !text_section || hunk_seen,
                "unsupported text section without hunks"
            );
            let old = path(old)?;
            let new = path(
                lines
                    .next()
                    .and_then(|s| s.strip_prefix("+++ "))
                    .context("missing +++ header")?,
            )?;
            ensure!(
                old == new || old == "/dev/null" || new == "/dev/null",
                "unsupported text rename; complete proposal not checked"
            );
            ensure!(
                old != "/dev/null" || new != "/dev/null",
                "diff section names no file"
            );
            if let Some((a, b)) = git.take() {
                ensure!(
                    (old == a || old == "/dev/null") && (new == b || new == "/dev/null"),
                    "unsupported Git/header mismatch"
                );
            }
            let deleted = new == "/dev/null";
            files.push((PathBuf::from(if deleted { old } else { new }), deleted));
            text_section = true;
            hunk_seen = false;
        } else if let Some(header) = line.strip_prefix("@@ -") {
            ensure!(text_section, "hunk before text section");
            let (old, new) = header.split_once(" +").context("invalid hunk header")?;
            let (new, _) = new.split_once(" @@").context("invalid hunk header")?;
            let count = |range: &str| -> Result<usize> {
                let (start, count) = range.split_once(',').unwrap_or((range, "1"));
                start.parse::<usize>().context("invalid hunk start")?;
                count.parse().context("invalid hunk count")
            };
            let (mut old, mut new) = (count(old)?, count(new)?);
            while old > 0 || new > 0 {
                let body = lines.next().context("truncated hunk")?;
                if body == "\\ No newline at end of file" {
                    continue;
                }
                let tag = body.as_bytes().first().copied().unwrap_or(b' ');
                match tag {
                    b' ' if old > 0 && new > 0 => {
                        old -= 1;
                        new -= 1;
                    }
                    b'-' if old > 0 => old -= 1,
                    b'+' if new > 0 => new -= 1,
                    _ => anyhow::bail!("invalid hunk body/counts; complete proposal not checked"),
                }
            }
            if lines.peek() == Some(&"\\ No newline at end of file") {
                lines.next();
            }
            hunk_seen = true;
        } else if let Some(mode) = line.strip_prefix("new file mode ") {
            ensure!(
                git.is_some() && mode == "100644",
                "unsupported Git file mode"
            );
        } else if let Some(mode) = line.strip_prefix("deleted file mode ") {
            ensure!(
                git.is_some() && matches!(mode, "100644" | "100755"),
                "unsupported Git resource mode"
            );
        } else if line.starts_with("index ") {
            ensure!(
                git.is_some()
                    && line
                        .split_whitespace()
                        .nth(2)
                        .is_none_or(|s| matches!(s, "100644" | "100755")),
                "unsupported Git index/resource mode"
            );
        } else {
            ensure!(
                line.is_empty(),
                "unsupported diff metadata/resource section: {line}; complete proposal not checked; no validation was performed"
            );
        }
    }
    ensure!(
        git.is_none() && text_section && hunk_seen,
        "unsupported Git metadata-only or empty proposal; no validation was performed"
    );
    Ok(files)
}
