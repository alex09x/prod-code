//! A unified diff applied in memory, so a proposed change can be validated in the form an agent
//! usually has it: a patch, not the whole of every file.
//!
//! Each file's hunks are applied to the file as it is on disk. A hunk is placed at the line it
//! names, or, when the file has moved since the diff was made, at the nearest place where its
//! old lines are found; a hunk whose old lines are nowhere is refused, naming it. `--- /dev/null`
//! creates a file and `+++ /dev/null` deletes one. Nothing is written.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

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

/// `@@ -12,5 +12,6 @@ …` → 12.
fn old_start(header: &str) -> Option<usize> {
    let rest = header.strip_prefix("@@ -")?;
    let number: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    number.parse().ok()
}

fn parse(diff: &str) -> Result<Vec<FilePatch>> {
    let mut files: Vec<FilePatch> = Vec::new();
    let mut lines = diff.lines().peekable();
    while let Some(line) = lines.next() {
        if let Some(old) = line.strip_prefix("--- ") {
            let new_line = lines
                .next()
                .filter(|l| l.starts_with("+++ "))
                .with_context(|| format!("`{line}` is not followed by a `+++` line"))?;
            files.push(FilePatch {
                old: diff_path(old),
                new: diff_path(&new_line[4..]),
                hunks: Vec::new(),
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
            while let Some(next) = lines.peek() {
                let tag = next.chars().next().unwrap_or(' ');
                match tag {
                    ' ' | '-' | '+' if !next.starts_with("--- ") && !next.starts_with("+++ ") => {
                        hunk.lines.push((tag, next[tag.len_utf8()..].to_string()));
                        lines.next();
                    }
                    '\\' => {
                        // It follows the line it is about.
                        if hunk.lines.last().is_some_and(|(t, _)| *t != '-') {
                            hunk.new_ends_without_newline = true;
                        }
                        lines.next();
                    }
                    _ if next.is_empty() => {
                        // An empty context line whose leading space an editor stripped.
                        hunk.lines.push((' ', String::new()));
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

/// What the files under `root` read after `diff`.
pub fn apply(root: &Path, diff: &str) -> Result<Patched> {
    let mut out = Patched::default();
    for file in parse(diff)? {
        match (&file.old, &file.new) {
            (Some(old), None) => out.deleted.push(root.join(old)),
            (old, Some(new)) => {
                let current = match old {
                    Some(old) => std::fs::read_to_string(root.join(old))
                        .with_context(|| format!("the diff changes {old}, which cannot be read"))?,
                    None => String::new(),
                };
                let text = apply_hunks(&current, &file.hunks, new)?;
                out.texts.push((root.join(new), text));
            }
            (None, None) => anyhow::bail!("a diff section names no file"),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hunk_is_applied_where_it_says_or_where_its_lines_moved() {
        let text = "a\nb\nc\nd\ne\n";
        let diff = "--- a/f.rs\n+++ b/f.rs\n@@ -2,2 +2,2 @@\n b\n-c\n+C\n";
        let files = parse(diff).unwrap();
        assert_eq!(
            apply_hunks(text, &files[0].hunks, "f.rs").unwrap(),
            "a\nb\nC\nd\ne\n"
        );
        // Two lines were added at the top since the diff was made.
        let moved = "x\ny\na\nb\nc\nd\ne\n";
        assert_eq!(
            apply_hunks(moved, &files[0].hunks, "f.rs").unwrap(),
            "x\ny\na\nb\nC\nd\ne\n"
        );
        let err = apply_hunks("a\nb\nq\n", &files[0].hunks, "f.rs").unwrap_err();
        assert!(
            format!("{err}").contains("hunk 1 of f.rs does not apply"),
            "{err}"
        );
    }

    #[test]
    fn several_hunks_follow_each_other_and_the_last_newline_is_kept_or_dropped() {
        let text = "1\n2\n3\n4\n5\n6\n";
        let diff = "--- a/f\n+++ b/f\n@@ -1,1 +1,2 @@\n 1\n+1.5\n@@ -5,2 +6,2 @@\n 5\n-6\n+six\n\\ No newline at end of file\n";
        let files = parse(diff).unwrap();
        assert_eq!(
            apply_hunks(text, &files[0].hunks, "f").unwrap(),
            "1\n1.5\n2\n3\n4\n5\nsix"
        );
    }

    #[test]
    fn a_new_file_and_a_deleted_one_are_named_and_paths_lose_their_prefix() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("gone.rs"), "x\n").unwrap();
        std::fs::write(dir.path().join("kept.rs"), "fn a() {}\n").unwrap();
        let diff = "diff --git a/new.rs b/new.rs\n--- /dev/null\n+++ b/new.rs\n@@ -0,0 +1,2 @@\n+pub fn n() {}\n+\n--- a/gone.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-x\n--- a/kept.rs\t2026-09-23\n+++ b/kept.rs\n@@ -1 +1 @@\n-fn a() {}\n+fn a() -> u8 { 1 }\n";
        let patched = apply(dir.path(), diff).unwrap();
        assert_eq!(
            patched.texts,
            vec![
                (dir.path().join("new.rs"), "pub fn n() {}\n\n".to_string()),
                (
                    dir.path().join("kept.rs"),
                    "fn a() -> u8 { 1 }\n".to_string()
                ),
            ]
        );
        assert_eq!(patched.deleted, vec![dir.path().join("gone.rs")]);
        assert!(apply(dir.path(), "no diff here").is_err());
        assert!(parse("@@ -1 +1 @@\n-a\n+b\n").is_err());
        assert!(parse("--- a/x\nnot plus\n").is_err());
        assert_eq!(diff_path("/dev/null"), None);
        assert_eq!(old_start("@@ -12,5 +12,6 @@ fn x"), Some(12));
    }
}
