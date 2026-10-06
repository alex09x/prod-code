/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::Path;

use super::symbols::is_scratch_path;

/// Changed line ranges per file: the working tree against `base` (HEAD by default), plus
/// untracked files as fully changed. A deleted file, or one whose mode alone changed, is listed
/// with no ranges, since none of its lines changed or are left; a change whose lines cannot be
/// placed (a binary file) is listed as the whole file.
pub fn changed_lines(root: &Path, base: Option<&str>) -> Result<BTreeMap<String, Vec<(u32, u32)>>> {
    Ok(diff_hunks(root, base)?
        .into_iter()
        .map(|(file, change)| {
            let ranges = match change {
                Change::Hunks(hunks) => hunks
                    .iter()
                    .map(|h| {
                        // A pure removal still touches the line it follows.
                        let end = h.start.saturating_add(h.added.max(1) - 1);
                        (h.start.max(1), end.max(1))
                    })
                    .collect(),
                Change::Unknown(_) => vec![(1, u32::MAX)],
            };
            (file, ranges)
        })
        .collect())
}

/// What the diff says about one changed file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Change {
    /// Every hunk of it, placed in the new text; none when the file was deleted or git counts
    /// no changed line (its mode alone changed).
    Hunks(Vec<Hunk>),
    /// A change whose lines cannot be placed: a binary file, or a path or a hunk that cannot be
    /// read. What it touches is unknown, never nothing.
    Unknown(String),
}

/// One hunk of the diff, placed in the new text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Hunk {
    /// 1-based first line it adds or rewrites; for a pure removal, the line the removed lines
    /// followed (0 at the top of the file).
    pub(crate) start: u32,
    /// Lines it adds or rewrites; 0 for a pure removal.
    pub(crate) added: u32,
    /// Lines it removes or rewrites; 0 for a pure addition.
    pub(crate) removed: u32,
}

impl Hunk {
    /// Whether it changes the function spanning lines `from..=to`.
    pub(crate) fn touches(&self, from: u32, to: u32) -> bool {
        if self.added == 0 {
            // Removed between `start` and the next line: inside only when both are.
            return from <= self.start && self.start < to;
        }
        let end = self.start.saturating_add(self.added - 1);
        self.start <= to && end >= from
    }

    /// Whether everything it changes lies inside the functions spanning `spans`, given the new
    /// text's `lines`. A pure removal must sit between two lines of one function: removed
    /// between two functions, it took something at module level with it. Of the lines a pure
    /// addition adds, a blank one changes nothing.
    pub(crate) fn inside(&self, spans: &[(u32, u32)], lines: &[&str]) -> bool {
        if self.added == 0 {
            return spans.iter().any(|&(from, to)| self.touches(from, to));
        }
        let last = self
            .start
            .saturating_add(self.added - 1)
            .min(lines.len() as u32);
        (self.start..=last).all(|n| {
            let blank = (n as usize)
                .checked_sub(1)
                .and_then(|i| lines.get(i))
                .is_some_and(|l| l.trim().is_empty());
            (self.removed == 0 && blank) || spans.iter().any(|&(from, to)| from <= n && n <= to)
        })
    }
}

/// Runs git in `root` and returns what it printed.
pub(crate) fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .with_context(|| format!("git {} failed to start", args.join(" ")))?;
    if !out.status.success() {
        anyhow::bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out.stdout)
}

/// A path git printed verbatim, and why its change cannot be placed when it is not UTF-8 (no
/// file can be opened by the name it would be reported under).
fn path_text(bytes: &[u8]) -> (String, Option<String>) {
    match std::str::from_utf8(bytes) {
        Ok(path) => (path.to_string(), None),
        Err(_) => (
            String::from_utf8_lossy(bytes).into_owned(),
            Some("its path is not UTF-8".to_string()),
        ),
    }
}

/// A path as a diff header prints it: bare (a name with a space is followed by a tab that is
/// not part of it), or in double quotes with C escapes (`\t`, `\"`, `\\`, `\303\274` for the
/// bytes of `ü`) when it holds anything else. `None` when the quoting cannot be read.
pub(crate) fn git_path(field: &[u8]) -> Option<Vec<u8>> {
    let Some(quoted) = field.strip_prefix(b"\"") else {
        let end = field.iter().rposition(|&b| b != b'\t').map_or(0, |i| i + 1);
        return Some(field[..end].to_vec());
    };
    let mut out = Vec::new();
    let mut bytes = quoted.iter().copied();
    while let Some(b) = bytes.next() {
        match b {
            b'"' => return Some(out),
            b'\\' => {
                let escaped = bytes.next()?;
                out.push(match escaped {
                    b'a' => 0x07,
                    b'b' => 0x08,
                    b't' => b'\t',
                    b'n' => b'\n',
                    b'v' => 0x0b,
                    b'f' => 0x0c,
                    b'r' => b'\r',
                    b'"' | b'\\' => escaped,
                    b'0'..=b'3' => {
                        let (second, third) = (bytes.next()?, bytes.next()?);
                        if !second.is_ascii_digit() || second > b'7' {
                            return None;
                        }
                        if !third.is_ascii_digit() || third > b'7' {
                            return None;
                        }
                        (escaped - b'0') * 64 + (second - b'0') * 8 + (third - b'0')
                    }
                    _ => return None,
                });
            }
            _ => out.push(b),
        }
    }
    None
}

/// The path a `--- ` or `+++ ` line names after its `prefix`: `Some(None)` for `/dev/null`,
/// `None` when it cannot be read.
pub(crate) fn header_path(field: &[u8], prefix: &[u8]) -> Option<Option<String>> {
    let path = git_path(field)?;
    if path == b"/dev/null" {
        return Some(None);
    }
    let path = path.strip_prefix(prefix)?;
    Some(Some(String::from_utf8_lossy(path).into_owned()))
}

/// The hunk a `@@ -a,b +c,d @@` header describes, given what follows its first `@@ `; `None`
/// when it is not one.
pub(crate) fn hunk_header(rest: &[u8]) -> Option<Hunk> {
    let mut words = rest.split(|&b| b == b' ');
    let (old, new, end) = (words.next()?, words.next()?, words.next()?);
    if end != b"@@" {
        return None;
    }
    let span = |word: &[u8], sign: u8| -> Option<(u32, u32)> {
        let spec = std::str::from_utf8(word.strip_prefix(&[sign])?).ok()?;
        let digits = |s: &str| {
            (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
                .then(|| s.parse::<u32>().ok())
                .flatten()
        };
        Some(match spec.split_once(',') {
            Some((start, count)) => (digits(start)?, digits(count)?),
            None => (digits(spec)?, 1),
        })
    };
    let ((_, removed), (start, added)) = (span(old, b'-')?, span(new, b'+')?);
    Some(Hunk {
        start,
        added,
        removed,
    })
}

/// The change of every file in the working tree against `base` (HEAD by default), untracked
/// files as one hunk that adds everything. Every file the diff names is listed: a deleted one
/// or one whose mode alone changed with no hunks, and one whose lines cannot be placed (a binary
/// file, a path or hunk that cannot be read) as [`Change::Unknown`], never as no change.
pub(crate) fn diff_hunks(root: &Path, base: Option<&str>) -> Result<BTreeMap<String, Change>> {
    let base = base.unwrap_or("HEAD");
    // Without rename detection a moved file is its old path deleted and its new path added:
    // what called into the old path must be accounted for. External diff drivers, text
    // conversion and the configured prefixes would change what is printed.
    let common = [
        "--no-renames",
        "--no-ext-diff",
        "--no-textconv",
        "--no-relative",
    ];
    // Every changed path with the lines git counts for it, NUL-terminated and so never quoted:
    // the list the hunks below must account for in full.
    let numstat = git(
        root,
        &[&["diff", "--numstat", "-z"][..], &common, &[base, "--"]].concat(),
    )?;
    let mut changes: BTreeMap<String, Change> = BTreeMap::new();
    let mut counted: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    for record in numstat.split(|&b| b == 0).filter(|r| !r.is_empty()) {
        let mut fields = record.splitn(3, |&b| b == b'\t');
        let (Some(added), Some(removed), Some(path)) =
            (fields.next(), fields.next(), fields.next())
        else {
            anyhow::bail!(
                "git diff --numstat printed a record it cannot read: {}",
                String::from_utf8_lossy(record)
            );
        };
        if path.is_empty() {
            anyhow::bail!("git diff --numstat reported a rename despite --no-renames");
        }
        let (path, unreadable_path) = path_text(path);
        let count = |field: &[u8]| std::str::from_utf8(field).ok()?.parse::<u64>().ok();
        let why = match (count(added), count(removed)) {
            _ if unreadable_path.is_some() => unreadable_path,
            (Some(added), Some(removed)) => {
                let total = counted.entry(path.clone()).or_default();
                total.0 += added;
                total.1 += removed;
                None
            }
            _ if added == b"-" && removed == b"-" => Some("it is a binary file".to_string()),
            _ => Some(format!(
                "git diff --numstat counts it as {}",
                String::from_utf8_lossy(record)
            )),
        };
        match why {
            Some(why) => {
                changes.insert(path, Change::Unknown(why));
            }
            None => {
                changes
                    .entry(path)
                    .or_insert_with(|| Change::Hunks(Vec::new()));
            }
        }
    }

    let diff = git(
        root,
        &[
            &[
                "diff",
                "-U0",
                "--no-color",
                "--src-prefix=a/",
                "--dst-prefix=b/",
            ][..],
            &common,
            &[base, "--"],
        ]
        .concat(),
    )?;
    let mut hunks: BTreeMap<String, Vec<Hunk>> = BTreeMap::new();
    let mut read: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    // The file the hunks that follow belong to, and whether it is left to place them in.
    let mut current: Option<(String, bool)> = None;
    let mut old: Option<String> = None;
    // Between `diff --git` and the first `@@`: a removed line reading `-- a/x` is not a header.
    let mut in_header = false;
    for line in diff.split(|&b| b == b'\n') {
        if line.starts_with(b"diff --git ") {
            in_header = true;
            current = None;
            old = None;
        } else if in_header && let Some(field) = line.strip_prefix(b"--- ") {
            old = header_path(field, b"a/").flatten();
        } else if in_header && let Some(field) = line.strip_prefix(b"+++ ") {
            current = match header_path(field, b"b/") {
                Some(Some(new)) => Some((new, true)),
                // Deleted: its hunks are counted, with no text left to place them in.
                Some(None) => old.take().map(|old| (old, false)),
                // Unreadable: its hunks go uncounted, so its file is not complete below.
                None => None,
            };
        } else if let Some(rest) = line.strip_prefix(b"@@ ") {
            in_header = false;
            let Some((file, placed)) = &current else {
                continue;
            };
            let Some(hunk) = hunk_header(rest) else {
                let why = format!(
                    "a hunk header cannot be read: {}",
                    String::from_utf8_lossy(line)
                );
                changes.insert(file.clone(), Change::Unknown(why));
                continue;
            };
            let total = read.entry(file.clone()).or_default();
            total.0 += u64::from(hunk.added);
            total.1 += u64::from(hunk.removed);
            if *placed {
                hunks.entry(file.clone()).or_default().push(hunk);
            }
        }
    }
    // A file whose hunks do not add up to the lines git counted for it lost some to a header
    // that could not be read.
    for (file, change) in changes.iter_mut() {
        let Change::Hunks(placed) = change else {
            continue;
        };
        let want = counted.get(file).copied().unwrap_or_default();
        let got = read.get(file).copied().unwrap_or_default();
        if want == got {
            *placed = hunks.remove(file).unwrap_or_default();
        } else {
            *change = Change::Unknown(format!(
                "git counts {} added and {} removed line(s), but its hunks that could be read add up to {} and {}",
                want.0, want.1, got.0, got.1
            ));
        }
    }
    for file in read.keys() {
        if !changes.contains_key(file) {
            let why = "the diff has hunks for it, but git's list of changed files does not name it";
            changes.insert(file.clone(), Change::Unknown(why.to_string()));
        }
    }

    // Untracked files count as entirely new. NUL-terminated, so never quoted.
    let status = git(
        root,
        &["status", "--porcelain=v1", "-z", "-uall", "--no-renames"],
    )?;
    let mut entries = status.split(|&b| b == 0);
    while let Some(entry) = entries.next() {
        let (Some(xy), Some(path)) = (entry.get(..2), entry.get(3..)) else {
            continue;
        };
        // A rename or a copy names its source in a field of its own.
        if xy.contains(&b'R') || xy.contains(&b'C') {
            entries.next();
            continue;
        }
        if xy != b"??" {
            continue;
        }
        let (path, unreadable_path) = path_text(path);
        // Untracked files in known scratch or cache directories (e.g. `.prod/`, `.scratch/`)
        // are tool scratchspaces or temporary copies, not repository changes (#760).
        if is_scratch_path(&path) {
            continue;
        }
        changes
            .entry(path)
            .or_insert_with(|| match unreadable_path {
                Some(why) => Change::Unknown(why),
                None => Change::Hunks(vec![Hunk {
                    start: 1,
                    added: u32::MAX,
                    removed: 0,
                }]),
            });
    }
    Ok(changes)
}
