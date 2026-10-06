/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::HashSet;
use std::ops::Range;
use std::path::Path;

use super::lsp::{Pos, Span};

/// Identifier-shaped tokens of a body, in order, deduplicated, with the 1-based line and the
/// 1-based column in UTF-16 code units of the first occurrence of each. Words inside line
/// comments and string literals are skipped: they cost a round trip and never resolve to
/// anything useful.
pub fn candidate_names(text: &str, start_line: u32) -> Vec<(String, u32, u32)> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let Some(line) = u32::try_from(i)
            .ok()
            .and_then(|i| start_line.checked_add(i))
        else {
            break;
        };
        let code = strip_noise(raw);
        let bytes = code.as_bytes();
        let mut col = 0usize;
        while col < bytes.len() {
            let c = bytes[col] as char;
            if !(c.is_ascii_alphabetic() || c == '_') {
                col += 1;
                continue;
            }
            let start = col;
            while col < bytes.len() {
                let c = bytes[col] as char;
                if c.is_ascii_alphanumeric() || c == '_' {
                    col += 1;
                } else {
                    break;
                }
            }
            let word = &code[start..col];
            if word.len() > 1 && !is_keyword(word) && seen.insert(word.to_string()) {
                // `strip_noise` keeps byte offsets, so `start` is the word's offset in `raw`.
                let Some(utf16) = u32::try_from(raw[..start].encode_utf16().count())
                    .ok()
                    .and_then(|c| c.checked_add(1))
                else {
                    continue;
                };
                out.push((word.to_string(), line, utf16));
            }
        }
    }
    out
}

/// Blanks out line comments and string literals so their words are not resolved. Every
/// character is replaced by as many spaces as it has bytes, so offsets stay those of `line`.
pub(crate) fn strip_noise(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let blank = |out: &mut String, c: char| out.push_str(&" ".repeat(c.len_utf8()));
    let mut chars = line.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_string = !in_string;
                out.push(' ');
            }
            '\\' if in_string => {
                out.push(' ');
                if let Some(next) = chars.next() {
                    blank(&mut out, next);
                }
            }
            '/' if !in_string && chars.peek() == Some(&'/') => {
                out.push_str(&" ".repeat(line.len() - out.len()));
                break;
            }
            '#' if !in_string => {
                out.push_str(&" ".repeat(line.len() - out.len()));
                break;
            }
            _ if in_string => blank(&mut out, c),
            _ => out.push(c),
        }
    }
    out
}

pub(crate) fn is_keyword(word: &str) -> bool {
    const KEYWORDS: &[&str] = &[
        // Rust
        "as",
        "async",
        "await",
        "break",
        "const",
        "continue",
        "crate",
        "dyn",
        "else",
        "enum",
        "extern",
        "false",
        "fn",
        "for",
        "if",
        "impl",
        "in",
        "let",
        "loop",
        "match",
        "mod",
        "move",
        "mut",
        "pub",
        "ref",
        "return",
        "self",
        "Self",
        "static",
        "struct",
        "super",
        "trait",
        "true",
        "type",
        "unsafe",
        "use",
        "where",
        "while",
        // Go, TypeScript, Python, C-family words that are not names either
        "func",
        "package",
        "import",
        "var",
        "range",
        "defer",
        "chan",
        "go",
        "interface",
        "map",
        "nil",
        "function",
        "class",
        "new",
        "this",
        "null",
        "undefined",
        "export",
        "def",
        "class_",
        "None",
        "True",
        "False",
        "elif",
        "pass",
        "raise",
        "with",
        "lambda",
        "int",
        "bool",
        "string",
        "str",
        "float",
        "void",
        "auto",
        "template",
        "namespace",
    ];
    KEYWORDS.contains(&word)
}

/// The byte range of each line of `text` without its line break: lines end at `\n`, and a `\r`
/// before it belongs to the break. A text ending in a line break has an empty last line, the
/// place a position just past the final break points at.
pub(crate) fn line_bounds(text: &str) -> Vec<Range<usize>> {
    let mut lines = Vec::new();
    let mut start = 0;
    for (at, _) in text.match_indices('\n') {
        let end = if text[start..at].ends_with('\r') {
            at - 1
        } else {
            at
        };
        lines.push(start..end);
        start = at + 1;
    }
    lines.push(start..text.len());
    lines
}

/// Whether the source holds `pos`: its line exists, and its character, in UTF-16 code units, is
/// at most the line's length and falls between characters, not inside a surrogate pair. The
/// message counts lines and columns from 1, as the report does.
pub(crate) fn check_pos(text: &str, lines: &[Range<usize>], pos: Pos) -> Result<(), String> {
    let (line, character) = (pos.line as usize, u64::from(pos.character));
    let at = format!("position {}:{}", u64::from(pos.line) + 1, character + 1);
    let bounds = lines.get(line).ok_or_else(|| {
        format!(
            "{at} is past the last line of the source, which has {} line(s)",
            lines.len()
        )
    })?;
    let mut units = 0u64;
    for c in text[bounds.clone()].chars() {
        if units >= character {
            break;
        }
        units += c.len_utf16() as u64;
    }
    match units.cmp(&character) {
        std::cmp::Ordering::Equal => Ok(()),
        std::cmp::Ordering::Greater => {
            Err(format!("{at} splits a surrogate pair on line {}", line + 1))
        }
        std::cmp::Ordering::Less => Err(format!(
            "{at} is past the end of line {}, which is {units} UTF-16 unit(s) long",
            line + 1
        )),
    }
}

pub(crate) fn check_span(text: &str, lines: &[Range<usize>], span: Span) -> Result<(), String> {
    check_pos(text, lines, span.start)?;
    check_pos(text, lines, span.end)
}

/// Text of lines `start..=end` (1-based, inclusive), joined by `\n`; `None` when the source
/// has no such lines.
pub(crate) fn lines_of(text: &str, lines: &[Range<usize>], start: u32, end: u32) -> Option<String> {
    let first = start.checked_sub(1)? as usize;
    let last = end.checked_sub(1)? as usize;
    if first > last {
        return None;
    }
    Some(
        lines
            .get(first..=last)?
            .iter()
            .map(|r| &text[r.clone()])
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

pub(crate) fn relative(root: &Path, file: &Path) -> String {
    file.strip_prefix(root)
        .unwrap_or(file)
        .to_string_lossy()
        .replace('\\', "/")
}
