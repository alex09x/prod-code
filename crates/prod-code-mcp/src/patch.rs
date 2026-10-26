/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A unified diff applied in memory, so a proposed change can be validated in the form an agent
//! usually has it: a patch, not the whole of every file.
//!
//! Each file's hunks are applied to the file as it is on disk. A hunk is placed at the line it
//! names, or, when the file has moved since the diff was made, at the nearest place where its
//! old lines are found; a hunk whose old lines are nowhere is refused, naming it. `--- /dev/null`
//! creates a file and `+++ /dev/null` deletes one. Nothing is written.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

mod deletion;

/// The files a patch touches, as they would read after it.
#[derive(Debug, Default, PartialEq)]
pub struct Patched {
    /// Every file the patch creates or changes, with its new text.
    pub texts: Vec<(PathBuf, String)>,
    /// Every file the patch deletes.
    pub deleted: Vec<PathBuf>,
}

/// One hunk: where its old lines start (1-based; 0 for an empty file) and its lines, each with
/// its tag (' ', '-' or '+').
#[derive(Debug)]
struct Hunk {
    old_start: usize,
    lines: Vec<(char, String)>,
    /// `\ No newline at end of file` after the last new line.
    new_ends_without_newline: bool,
}

/// One file's section of a diff.
#[derive(Debug)]
struct FilePatch {
    old: Option<String>,
    new: Option<String>,
    hunks: Vec<Hunk>,
    old_ends_without_newline: bool,
}

/// `a/src/x.rs` → `src/x.rs`; `/dev/null` → `None`.
fn diff_path(raw: &str) -> Option<String> {
    let path = raw.split('\t').next().unwrap_or(raw).trim();
    if path == "/dev/null" {
        return None;
    }
    Some(
        path.strip_prefix("a/")
            .or_else(|| path.strip_prefix("b/"))
            .unwrap_or(path)
            .to_string(),
    )
}

/// `@@ -12,5 +12,6 @@ …` → (5, 6): how many old and new lines the hunk says it has; a count
/// left out is 1.
fn hunk_counts(header: &str) -> Option<(usize, usize)> {
    let rest = header.strip_prefix("@@ -")?;
    let (old, rest) = rest.split_once(" +")?;
    let new = rest.split_once(" @@").map_or(rest, |(new, _)| new);
    let count = |range: &str| {
        range
            .split_once(',')
            .map_or(Some(1), |(_, count)| count.trim().parse().ok())
    };
    Some((count(old)?, count(new)?))
}

/// `@@ -12,5 +12,6 @@ …` → 12.
fn old_start(header: &str) -> Option<usize> {
    let rest = header.strip_prefix("@@ -")?;
    let number: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    number.parse().ok()
}

/// Match `str::lines` for ordinary patch lines; deletion bodies use raw CR bytes.
fn diff_line(raw: &str) -> &str {
    raw.strip_suffix('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .unwrap_or(raw)
}

fn parse(diff: &str) -> Result<Vec<FilePatch>> {
    let mut files: Vec<FilePatch> = Vec::new();
    let mut lines = diff.split_inclusive('\n').peekable();
    while let Some(raw) = lines.next() {
        let line = diff_line(raw);
        if let Some(old) = line.strip_prefix("--- ") {
            let new_line = lines
                .next()
                .map(diff_line)
                .filter(|l| l.starts_with("+++ "))
                .with_context(|| format!("`{line}` is not followed by a `+++` line"))?;
            let new = diff_path(&new_line[4..]);
            let (hunks, old_ends_without_newline) = if new.is_none() {
                deletion::parse_hunks(&mut lines)?
            } else {
                (Vec::new(), false)
            };
            files.push(FilePatch {
                old: diff_path(old),
                new,
                hunks,
                old_ends_without_newline,
            });
            continue;
        }
        if line.starts_with("@@ ") {
            let file = files
                .last_mut()
                .context("a hunk comes before any `---`/`+++` header")?;
            let start =
                old_start(line).with_context(|| format!("`{line}` is not a hunk header"))?;
            let mut hunk = Hunk {
                old_start: start,
                lines: Vec::new(),
                new_ends_without_newline: false,
            };
            let counts = hunk_counts(line);
            let (mut old_lines, mut new_lines) = (0, 0);
            while let Some(&raw) = lines.peek() {
                let next = diff_line(raw);
                if counts == Some((old_lines, new_lines)) && !next.starts_with('\\') {
                    break;
                }
                if next.is_empty() {
                    // An empty context line whose leading space an editor stripped, or a blank
                    // line after the diff; the header's counts tell them apart below.
                    hunk.lines.push((' ', String::new()));
                    old_lines += 1;
                    new_lines += 1;
                    lines.next();
                    continue;
                }
                let tag = next.chars().next().unwrap_or(' ');
                match tag {
                    ' ' | '-' | '+' => {
                        hunk.lines.push((tag, next[tag.len_utf8()..].to_string()));
                        old_lines += usize::from(tag != '+');
                        new_lines += usize::from(tag != '-');
                        lines.next();
                    }
                    '\\' => {
                        // It follows the line it is about.
                        if hunk.lines.last().is_some_and(|(t, _)| *t != '-') {
                            hunk.new_ends_without_newline = true;
                        }
                        lines.next();
                    }
                    _ => break,
                }
            }
            file.hunks.push(hunk);
        }
    }
    anyhow::ensure!(
        !files.is_empty(),
        "no file in the diff (no `---`/`+++` lines)"
    );
    Ok(files)
}

/// `text` with `hunks` applied, or the hunk that does not fit.
fn apply_hunks(text: &str, hunks: &[Hunk], shown: &str) -> Result<String> {
    let ended_with_newline = text.is_empty() || text.ends_with('\n');
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let mut shift: isize = 0;
    let mut floor = 0usize;
    let mut no_newline = !ended_with_newline;
    for (n, hunk) in hunks.iter().enumerate() {
        let old: Vec<&str> = hunk
            .lines
            .iter()
            .filter(|(t, _)| *t != '+')
            .map(|(_, l)| l.as_str())
            .collect();
        let new: Vec<String> = hunk
            .lines
            .iter()
            .filter(|(t, _)| *t != '-')
            .map(|(_, l)| l.clone())
            .collect();
        let expected = ((hunk.old_start.max(1) as isize - 1) + shift).max(0) as usize;
        let fits = |at: usize| {
            at >= floor
                && at + old.len() <= lines.len()
                && lines[at..at + old.len()]
                    .iter()
                    .zip(&old)
                    .all(|(a, b)| a == b)
        };
        let at = if old.is_empty() {
            Some(expected.min(lines.len()))
        } else {
            (0..=lines.len())
                .filter(|at| fits(*at))
                .min_by_key(|at| at.abs_diff(expected))
        }
        .with_context(|| {
            format!(
                "hunk {} of {shown} does not apply: its old lines (from line {}) are not in the file",
                n + 1,
                hunk.old_start
            )
        })?;
        let tail_touched = at + old.len() == lines.len();
        lines.splice(at..at + old.len(), new.iter().cloned());
        shift += new.len() as isize - old.len() as isize;
        floor = at + new.len();
        if tail_touched {
            no_newline = hunk.new_ends_without_newline;
        }
    }
    let mut out = lines.join("\n");
    if !lines.is_empty() && !no_newline {
        out.push('\n');
    }
    Ok(out)
}

fn git_read_blob(root: &Path, rel_path: &str) -> Option<String> {
    let clean = rel_path.trim_start_matches("./");
    for spec in [
        format!(":./{clean}"),
        format!("HEAD:./{clean}"),
        format!(":{clean}"),
        format!("HEAD:{clean}"),
    ] {
        if let Ok(output) = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .arg("show")
            .arg(&spec)
            .output()
            && output.status.success()
            && let Ok(s) = String::from_utf8(output.stdout)
        {
            return Some(s);
        }
    }
    None
}

/// What the files under `root` read after `diff`.
pub fn apply(root: &Path, diff: &str) -> Result<Patched> {
    let mut out = Patched::default();
    for file in parse(diff)? {
        match (&file.old, &file.new) {
            (Some(old), None) => {
                let path = root.join(old);
                deletion::validate(&path, old, &file.hunks, file.old_ends_without_newline)?;
                out.deleted.push(path);
            }
            (old, Some(new)) => {
                let current = match old {
                    Some(old) => std::fs::read_to_string(root.join(old))
                        .with_context(|| format!("the diff changes {old}, which cannot be read"))?,
                    None => String::new(),
                };
                let text = match apply_hunks(&current, &file.hunks, new) {
                    Ok(text) => text,
                    Err(orig_err) => {
                        if let Some(old) = old
                            && let Some(base) = git_read_blob(root, old)
                            && let Ok(text) = apply_hunks(&base, &file.hunks, new)
                        {
                            text
                        } else {
                            return Err(orig_err);
                        }
                    }
                };
                out.texts.push((root.join(new), text));
            }
            (None, None) => anyhow::bail!("a diff section names no file"),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
