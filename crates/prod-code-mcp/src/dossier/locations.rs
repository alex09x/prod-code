/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::BTreeSet;
use std::path::Path;

/// `file:line` locations mentioned in a failure's output that lie inside the checkout, in
/// order of appearance, deduplicated: Rust panics and `-->` notes, Go `file.go:12:`, Python
/// `File "x.py", line 12`, JS/TS `(x.ts:12:5)`, Swift/C `x.swift:12: error`.
pub fn locations_in(root: &Path, text: &str) -> Vec<(String, u32)> {
    locations_in_with_hint(root, text, "")
}

/// [`locations_in`] with a hint (the failing test's name, e.g. `pkg/sub.TestX`) used to
/// resolve bare file names such as Go's `signal_test.go:7` to the right directory.
pub fn locations_in_with_hint(root: &Path, text: &str, hint: &str) -> Vec<(String, u32)> {
    let root_str = std::fs::canonicalize(root)
        .unwrap_or_else(|_| root.to_path_buf())
        .to_string_lossy()
        .into_owned();
    // Every source file of the checkout, for resolving bare file names.
    let all_files: Vec<String> = crate::sync::scan_workspace_files(root, None)
        .map(|files| files.into_iter().map(|f| f.relative_path).collect())
        .unwrap_or_default();
    let hint_segments: Vec<&str> = hint
        .split(['/', '.', ':'])
        .filter(|s| !s.is_empty())
        .collect();
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    let mut in_python_traceback = false;
    let mut python_frame_indices = Vec::new();
    let mut push = |file: &str, line: u32| -> Option<usize> {
        let file = file.trim_matches(|c| c == '"' || c == '(' || c == ')' || c == '\'');
        let mut rel = file
            .strip_prefix(&format!("{root_str}/"))
            .unwrap_or(file)
            .to_string();
        if rel.starts_with('/')
            || rel.starts_with("..")
            || rel.contains("/.cargo/")
            || rel.contains("/rustlib/")
        {
            return None;
        }
        if !root.join(&rel).is_file() {
            // A bare name: the unique file with that basename, or the one whose directory
            // matches the test's package.
            if rel.contains('/') {
                return None;
            }
            let candidates: Vec<&String> = all_files
                .iter()
                .filter(|p| p.rsplit('/').next() == Some(rel.as_str()))
                .collect();
            let chosen = match candidates.as_slice() {
                [] => return None,
                [one] => (*one).clone(),
                many => many
                    .iter()
                    .find(|p| {
                        hint_segments
                            .iter()
                            .any(|seg| p.split('/').any(|part| part == *seg))
                    })
                    .map(|p| (*p).clone())
                    .unwrap_or_else(|| (*many[0]).clone()),
            };
            rel = chosen;
        }
        if seen.insert((rel.clone(), line)) {
            let index = out.len();
            out.push((rel, line));
            Some(index)
        } else {
            None
        }
    };
    for raw in text.lines() {
        if raw.trim() == "Traceback (most recent call last):" {
            in_python_traceback = true;
        }
        // Python: File "path", line N
        if let Some(rest) = raw.trim().strip_prefix("File \"")
            && let Some((file, rest)) = rest.split_once("\", line ")
            && let Some(n) = rest
                .split(|c: char| !c.is_ascii_digit())
                .next()
                .and_then(|n| n.parse().ok())
        {
            if let Some(index) = push(file, n) {
                if in_python_traceback {
                    python_frame_indices.push(index);
                }
            }
            continue;
        }
        // Everything else: tokens shaped path:line[:col]
        for token in raw.split(|c: char| c.is_whitespace() || c == '(' || c == ')') {
            let mut parts = token.splitn(3, ':');
            let (Some(file), Some(line)) = (parts.next(), parts.next()) else {
                continue;
            };
            if !file.contains('.') || file.starts_with("http") {
                continue;
            }
            let Ok(n) = line
                .trim_end_matches(|c: char| !c.is_ascii_digit())
                .parse::<u32>()
            else {
                continue;
            };
            if n == 0 {
                continue;
            }
            let _ = push(file, n);
        }
    }
    drop(push);
    if in_python_traceback && let Some(deepest) = python_frame_indices.last().copied() {
        let location = out.remove(deepest);
        out.insert(0, location);
    }
    out
}

pub(crate) fn truncate_utf8(s: &mut String, max_bytes: usize) {
    if s.len() > max_bytes {
        let mut cut = max_bytes;
        while cut > 0 && !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s.push_str("\n…");
    }
}

pub(crate) fn git_diff_of(root: &Path, file: &str) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["diff", "-U3", "--no-color", "HEAD", "--", file])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let hunks: Vec<&str> = text.lines().skip_while(|l| !l.starts_with("@@")).collect();
    if hunks.is_empty() {
        None
    } else {
        let mut s = hunks.join("\n");
        truncate_utf8(&mut s, 4000);
        Some(s)
    }
}

/// The error-level fixes among `fixes`, one line each: `file:line: message`.
pub fn suggested(fixes: &[crate::fixit::Fix]) -> Vec<String> {
    let mut out: Vec<String> = fixes
        .iter()
        .filter(|f| f.level == "error")
        .filter_map(|f| {
            let first = f.edits.first()?;
            Some(format!(
                "{}:{}: {}",
                first.file,
                first.line,
                f.message.lines().next().unwrap_or("")
            ))
        })
        .collect();
    out.dedup();
    out
}
