/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};

use super::ops::{MultiOp, multi_operations};

/// Applies LSP text edits (0-based line/character, character counted in UTF-16 code units, the
/// LSP default; prod-code negotiates no other position encoding) to `text`. A single edit
/// starting at 0:0 and ending at or past the last line replaces the whole file.
pub fn apply_text_edits(text: &str, edits: &[serde_json::Value]) -> Result<String> {
    apply_edits_counting(text, edits, char::len_utf16)
}

/// [`apply_text_edits`] for edits whose character counts Unicode scalar values, the columns the
/// crate's own text scanners produce rather than an analyzer's.
pub fn apply_scalar_text_edits(text: &str, edits: &[serde_json::Value]) -> Result<String> {
    apply_edits_counting(text, edits, |_| 1)
}

fn apply_edits_counting(
    text: &str,
    edits: &[serde_json::Value],
    width: fn(char) -> usize,
) -> Result<String> {
    let line_count = text.lines().count() as u64;
    let is_full_replacement = if let [edit] = edits
        && edit.pointer("/range/start/line").and_then(|v| v.as_u64()) == Some(0)
        && edit
            .pointer("/range/start/character")
            .and_then(|v| v.as_u64())
            == Some(0)
    {
        let end_line = edit
            .pointer("/range/end/line")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let end_char = edit
            .pointer("/range/end/character")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        end_line >= line_count
            || (end_line == line_count.saturating_sub(1)
                && end_char
                    >= text
                        .lines()
                        .last()
                        .map(|l| l.chars().map(width).sum::<usize>() as u64)
                        .unwrap_or(0))
    } else {
        false
    };
    if is_full_replacement {
        return Ok(edits[0]
            .get("newText")
            .and_then(|t| t.as_str())
            .unwrap_or_default()
            .to_string());
    }
    // General case: convert positions to byte offsets and apply from the end backwards.
    let line_starts: Vec<usize> = std::iter::once(0)
        .chain(
            text.char_indices()
                .filter(|(_, c)| *c == '\n')
                .map(|(i, _)| i + 1),
        )
        .collect();
    // A column past the end of its line is the line's end; one inside a character (half a
    // surrogate pair) is the character's end.
    let offset = |line: u64, character: u64| -> usize {
        let start = line_starts
            .get(line as usize)
            .copied()
            .unwrap_or(text.len());
        let rest = &text[start..];
        let end_of_line = rest.find('\n').unwrap_or(rest.len());
        let mut units = 0u64;
        for (i, c) in rest[..end_of_line].char_indices() {
            if units >= character {
                return start + i;
            }
            units += width(c) as u64;
        }
        start + end_of_line
    };
    let mut spans: Vec<(usize, usize, String)> = edits
        .iter()
        .map(|e| {
            let sl = e
                .pointer("/range/start/line")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let sc = e
                .pointer("/range/start/character")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let el = e
                .pointer("/range/end/line")
                .and_then(|v| v.as_u64())
                .unwrap_or(sl);
            let ec = e
                .pointer("/range/end/character")
                .and_then(|v| v.as_u64())
                .unwrap_or(sc);
            let new_text = e
                .get("newText")
                .and_then(|t| t.as_str())
                .unwrap_or_default()
                .to_string();
            (offset(sl, sc), offset(el, ec), new_text)
        })
        .collect();
    spans.sort_by_key(|span| std::cmp::Reverse(span.0));
    let mut out = text.to_string();
    for (start, end, new_text) in spans {
        if start > end || end > out.len() {
            return Err(anyhow!("text edit range {start}..{end} out of bounds"));
        }
        out.replace_range(start..end, &new_text);
    }
    Ok(out)
}

/// The bytes of the file at `abs`, or `None` where there is no file and one could be created
/// (nothing there, or a file where a directory above it would be). Any other failure is an
/// error: a file that exists but cannot be read is not an empty one.
pub fn read_existing(abs: &Path) -> Result<Option<Vec<u8>>> {
    match std::fs::read(abs) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(err)
            if matches!(
                err.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(None)
        }
        Err(err) => Err(anyhow!(err).context(format!("cannot read {}", abs.display()))),
    }
}

/// The text of the file at `abs` for editing: empty where there is no file yet.
pub fn text_for_edit(abs: &Path) -> Result<(Option<Vec<u8>>, String)> {
    let bytes = read_existing(abs)?;
    let text = match &bytes {
        Some(b) => String::from_utf8(b.clone())
            .map_err(|_| anyhow!("{} is not UTF-8 text; it cannot be edited", abs.display()))?,
        None => String::new(),
    };
    Ok((bytes, text))
}

/// What every file an edit rewrites would contain, without writing anything: the operations of
/// a `WorkspaceEdit` applied in memory, in order, to the files as they are. A renamed file's
/// text moves to its new path and later edits of that path apply to it, and a created one starts
/// empty (#1469). A deleted or moved-away path is left out of the result, since an overlay
/// cannot remove a file. This is a preview, not the transactional apply
/// [`crate::refactor::apply_workspace_edit`] makes. The second value says whether the edit had
/// any resource operation, so a caller can say that the removal of files was not checked.
pub fn planned_texts(
    root: &Path,
    edit: &serde_json::Value,
) -> Result<(Vec<(PathBuf, String)>, bool)> {
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    planned_multi_texts(&[&root], edit)
}

/// [`planned_texts`] across multiple repository roots.
pub fn planned_multi_texts(
    roots: &[&Path],
    edit: &serde_json::Value,
) -> Result<(Vec<(PathBuf, String)>, bool)> {
    let canonical_roots: Vec<PathBuf> = roots
        .iter()
        .map(|r| std::fs::canonicalize(r).unwrap_or_else(|_| r.to_path_buf()))
        .collect();
    // Each path's proposed text, in the order the edit first names it, and the paths it removed.
    let mut out: Vec<(PathBuf, String)> = Vec::new();
    let mut gone: Vec<PathBuf> = Vec::new();
    let mut moves_files = false;
    let current = |out: &[(PathBuf, String)], gone: &[PathBuf], abs: &Path| -> Result<String> {
        if let Some((_, text)) = out.iter().find(|(p, _)| p == abs) {
            return Ok(text.clone());
        }
        if gone.iter().any(|p| abs.starts_with(p)) {
            return Ok(String::new());
        }
        Ok(text_for_edit(abs)?.1)
    };
    let set = |out: &mut Vec<(PathBuf, String)>, gone: &mut Vec<PathBuf>, abs: PathBuf, text| {
        gone.retain(|p| *p != abs);
        match out.iter_mut().find(|(p, _)| *p == abs) {
            Some((_, slot)) => *slot = text,
            None => out.push((abs, text)),
        }
    };
    for op in multi_operations(&canonical_roots, edit)? {
        match op {
            MultiOp::Text { root, rel, edits } => {
                let abs = root.join(&rel);
                let text = apply_text_edits(&current(&out, &gone, &abs)?, &edits)?;
                set(&mut out, &mut gone, abs, text);
            }
            MultiOp::Create { root, rel, .. } => {
                moves_files = true;
                let abs = root.join(&rel);
                let text = current(&out, &gone, &abs)?;
                set(&mut out, &mut gone, abs, text);
            }
            MultiOp::Rename {
                from_root,
                from_rel,
                to_root,
                to_rel,
                ..
            } => {
                moves_files = true;
                let from = from_root.join(&from_rel);
                let text = current(&out, &gone, &from)?;
                out.retain(|(p, _)| *p != from);
                gone.push(from);
                set(&mut out, &mut gone, to_root.join(&to_rel), text);
            }
            MultiOp::Delete { root, rel, .. } => {
                moves_files = true;
                let abs = root.join(&rel);
                out.retain(|(p, _)| !p.starts_with(&abs));
                gone.push(abs);
            }
        }
    }
    Ok((out, moves_files))
}
