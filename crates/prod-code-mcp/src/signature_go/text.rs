/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::signature_go::types::TextEdit;
use std::path::Path;

pub(crate) fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// `file:line:column`, 1-based, of a byte offset, as a message names a place.
pub(crate) fn position(root: &Path, file: &Path, text: &str, offset: usize) -> String {
    let (l, c) = line_col_utf16(text, offset);
    format!("{}:{}:{}", display(root, file), l + 1, c + 1)
}

/// The 0-based line and UTF-16 column of a byte offset.
pub(crate) fn line_col_utf16(text: &str, offset: usize) -> (u32, u32) {
    let before = &text[..offset.min(text.len())];
    let line = before.matches('\n').count() as u32;
    let col = before
        .rsplit('\n')
        .next()
        .map_or(0, |l| l.chars().map(|c| c.len_utf16() as u32).sum());
    (line, col)
}

/// The byte offset of a 0-based line and UTF-16 column, the positions gopls speaks in.
pub(crate) fn offset_at(text: &str, line: u32, col: u32) -> Option<usize> {
    let mut start = 0usize;
    for _ in 0..line {
        start += text[start..].find('\n')? + 1;
    }
    let rest = &text[start..];
    let line_end = rest.find('\n').unwrap_or(rest.len());
    let end = line_end - usize::from(line_end < rest.len() && rest[..line_end].ends_with('\r'));
    let mut units = 0u32;
    for (i, ch) in rest[..end].char_indices() {
        if units >= col {
            return (units == col).then_some(start + i);
        }
        units += ch.len_utf16() as u32;
    }
    (units == col).then_some(start + end)
}

pub(crate) fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

/// Where the string, rune or comment that starts at `i` ends; `None` when none starts there.
pub(crate) fn skip_opaque(s: &[u8], i: usize) -> Option<usize> {
    match s[i] {
        quote @ (b'"' | b'\'') => {
            let mut j = i + 1;
            while j < s.len() {
                match s[j] {
                    b'\\' => j += 2,
                    b'\n' => return Some(j),
                    c if c == quote => return Some(j + 1),
                    _ => j += 1,
                }
            }
            Some(s.len())
        }
        b'`' => Some(
            s[i + 1..]
                .iter()
                .position(|&c| c == b'`')
                .map_or(s.len(), |p| i + p + 2),
        ),
        b'/' if s.get(i + 1) == Some(&b'/') => Some(
            s[i..]
                .iter()
                .position(|&c| c == b'\n')
                .map_or(s.len(), |p| i + p),
        ),
        b'/' if s.get(i + 1) == Some(&b'*') => Some(
            s[i + 2..]
                .windows(2)
                .position(|w| w == b"*/")
                .map_or(s.len(), |p| i + p + 4),
        ),
        _ => None,
    }
}

/// The offset of the bracket that closes the one at `open`.
pub(crate) fn closing(text: &str, open: usize) -> Option<usize> {
    let s = text.as_bytes();
    let mut depth = 0i32;
    let mut i = open;
    while i < s.len() {
        if let Some(end) = skip_opaque(s, i) {
            i = end;
            continue;
        }
        match s[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
                if depth < 0 {
                    return None;
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Skips whitespace and comments from `i`.
pub(crate) fn skip_space(text: &str, mut i: usize) -> usize {
    let s = text.as_bytes();
    while i < s.len() {
        if s[i].is_ascii_whitespace() {
            i += 1;
        } else if s[i] == b'/' && matches!(s.get(i + 1), Some(b'/' | b'*')) {
            i = skip_opaque(s, i).unwrap_or(s.len());
        } else {
            break;
        }
    }
    i
}

/// The pieces of a list between its top-level commas, trimmed; a trailing comma adds none.
pub(crate) fn split_list(text: &str) -> Vec<String> {
    let s = text.as_bytes();
    let (mut depth, mut start, mut i) = (0i32, 0usize, 0usize);
    let mut out = Vec::new();
    while i < s.len() {
        if let Some(end) = skip_opaque(s, i) {
            i = end;
            continue;
        }
        match s[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b',' if depth == 0 => {
                out.push(text[start..i].trim().to_string());
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    let last = text[start..].trim();
    if !last.is_empty() {
        out.push(last.to_string());
    }
    out
}

/// `text` with every comment replaced by a space.
pub(crate) fn strip_comments(text: &str) -> String {
    let s = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let (mut i, mut copied) = (0usize, 0usize);
    while i < s.len() {
        if let Some(end) = skip_opaque(s, i) {
            if s[i] == b'/' {
                out.push_str(&text[copied..i]);
                out.push(' ');
                copied = end;
            }
            i = end;
            continue;
        }
        i += 1;
    }
    out.push_str(&text[copied..]);
    out
}

/// Every comment of a Go text, in order and as written.
pub(crate) fn comments(text: &str) -> Vec<&str> {
    let s = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        if let Some(end) = skip_opaque(s, i) {
            if s[i] == b'/' {
                out.push(&text[i..end]);
            }
            i = end;
            continue;
        }
        i += 1;
    }
    out
}

/// `text` with sorted, non-overlapping edits applied.
pub(crate) fn splice(text: &str, edits: &[TextEdit]) -> String {
    let mut out = text.to_string();
    for (s, e, t) in edits.iter().rev() {
        out.replace_range(*s..*e, t);
    }
    out
}

/// Where the byte at `at` of the old text is in the edited one; `None` when an edit replaces it.
pub(crate) fn map_offset(edits: &[TextEdit], at: usize) -> Option<usize> {
    let mut shifted = at as isize;
    for (s, e, t) in edits {
        if *e <= at {
            shifted += t.len() as isize - (*e - *s) as isize;
        } else if *s <= at {
            return None;
        }
    }
    usize::try_from(shifted).ok()
}
