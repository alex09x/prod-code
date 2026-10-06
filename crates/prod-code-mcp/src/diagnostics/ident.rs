/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

/// A Rust identifier token found in executable source code (outside comments, strings,
/// character literals, numbers and lifetimes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustIdent {
    pub name: String,
    pub line: u32,
    pub col: u32,
}

/// Converts a 0-based byte offset in `text` to 1-based (line, UTF-16 column).
pub fn byte_to_line_col(line_starts: &[usize], text: &str, byte_offset: usize) -> (u32, u32) {
    let line_idx = line_starts
        .partition_point(|&s| s <= byte_offset)
        .saturating_sub(1);
    let line_no = line_idx as u32 + 1;
    let line_start = line_starts[line_idx];
    let col = text[line_start..byte_offset]
        .chars()
        .map(|c| c.len_utf16())
        .sum::<usize>() as u32
        + 1;
    (line_no, col)
}

/// Checks whether a raw string (e.g. `r"..."`, `r#"..."#`, `br#"..."#`, `cr#"..."#`) starts at `i`.
/// Returns `Some((content_start_char_idx, num_hashes))` if so.
pub fn raw_string_start(chars: &[(usize, char)], i: usize) -> Option<(usize, usize)> {
    let at = |idx: usize| chars.get(idx).map(|&(_, c)| c);
    let mut p = i;
    if matches!(at(p), Some('b') | Some('c')) && at(p + 1) == Some('r') {
        p += 2;
    } else if at(p) == Some('r') {
        p += 1;
    } else {
        return None;
    }
    let mut hashes = 0;
    while at(p) == Some('#') {
        hashes += 1;
        p += 1;
    }
    if at(p) == Some('"') {
        Some((p + 1, hashes))
    } else {
        None
    }
}

/// Checks whether a quoted string (`"..."`, `b"..."`, `c"..."`) starts at `i`.
/// Returns `Some(content_start_char_idx)` if so.
pub fn quoted_string_start(chars: &[(usize, char)], i: usize) -> Option<usize> {
    let at = |idx: usize| chars.get(idx).map(|&(_, c)| c);
    if at(i) == Some('"') {
        Some(i + 1)
    } else if matches!(at(i), Some('b') | Some('c')) && at(i + 1) == Some('"') {
        Some(i + 2)
    } else {
        None
    }
}

/// Scans `text` for Rust identifier tokens, skipping whitespace, line/doc/nested block
/// comments, string literals (ordinary, raw, byte, C), character literals, numbers,
/// and lifetime identifiers. Returns identifier tokens with their 1-based line and 1-based
/// UTF-16 column.
pub fn rust_code_identifiers(text: &str) -> Vec<RustIdent> {
    let line_starts: Vec<usize> = std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .collect();

    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let at = |idx: usize| chars.get(idx).map(|&(_, c)| c);
    let offset = |idx: usize| chars.get(idx).map_or(text.len(), |&(o, _)| o);
    let ident_start = |c: char| c == '_' || unicode_ident::is_xid_start(c);
    let ident_char = |c: char| c == '_' || unicode_ident::is_xid_continue(c);

    let mut tokens = Vec::new();
    let mut i = 0;

    while let Some(c) = at(i) {
        if c.is_whitespace() {
            i += 1;
        } else if c == '/' && at(i + 1) == Some('/') {
            // Line comment (including /// doc comments and //! inner doc comments)
            i += 2;
            while at(i).is_some_and(|ch| ch != '\n') {
                i += 1;
            }
        } else if c == '/' && at(i + 1) == Some('*') {
            // Block comment (including /** doc comments and nested /* /* */ */)
            let mut depth = 1usize;
            i += 2;
            while i < chars.len() && depth > 0 {
                if at(i) == Some('/') && at(i + 1) == Some('*') {
                    depth += 1;
                    i += 2;
                } else if at(i) == Some('*') && at(i + 1) == Some('/') {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
        } else if let Some((content_start, hashes)) = raw_string_start(&chars, i) {
            // Raw string (r"...", r#"..."#, br"...", br#"..."#, cr"...", cr#"..."#)
            let mut p = content_start;
            while p < chars.len() {
                if at(p) == Some('"') && (1..=hashes).all(|k| at(p + k) == Some('#')) {
                    p += 1 + hashes;
                    break;
                }
                p += 1;
            }
            i = p;
        } else if let Some(content_start) = quoted_string_start(&chars, i) {
            // Quoted string ("...", b"...", c"...")
            let mut p = content_start;
            while p < chars.len() {
                match at(p) {
                    Some('\\') => p += 2,
                    Some('"') => {
                        p += 1;
                        break;
                    }
                    _ => p += 1,
                }
            }
            i = p;
        } else if c == 'b' && at(i + 1) == Some('\'') {
            // Byte character literal: b'a', b'\'', b'\\'
            let mut j = i + 2;
            if at(j) == Some('\\') {
                j += 1;
                while at(j).is_some_and(|ch| ch != '\'' && ch != '\n') {
                    j += 1;
                }
                if at(j) == Some('\'') {
                    j += 1;
                }
            } else if at(j).is_some() && at(j + 1) == Some('\'') && at(j) != Some('\n') {
                j += 2;
            } else {
                while at(j).is_some_and(|ch| ch != '\'' && ch != '\n') {
                    j += 1;
                }
                if at(j) == Some('\'') {
                    j += 1;
                }
            }
            i = j;
        } else if c == '\'' {
            // Character literal vs lifetime
            if at(i + 1) == Some('\\') {
                // Escaped char literal: '\'', '\\', '\n', '\u{1F600}'
                let mut j = i + 2;
                while at(j).is_some_and(|ch| ch != '\'' && ch != '\n') {
                    j += 1;
                }
                if at(j) == Some('\'') {
                    j += 1;
                }
                i = j;
            } else if at(i + 1).is_some() && at(i + 2) == Some('\'') && at(i + 1) != Some('\n') {
                // Single character literal: 'a', '0', ' '
                i += 3;
            } else if at(i + 1).is_some_and(ident_start) {
                // Lifetime identifier: 'static, 'a, 'r#life
                i += 1;
                if at(i) == Some('r')
                    && at(i + 1) == Some('#')
                    && at(i + 2).is_some_and(ident_start)
                {
                    i += 2;
                }
                while at(i).is_some_and(ident_char) {
                    i += 1;
                }
            } else {
                i += 1;
            }
        } else if c == 'r' && at(i + 1) == Some('#') && at(i + 2).is_some_and(ident_start) {
            // Raw identifier: r#foo
            let token_start = offset(i);
            i += 2;
            let name_start = offset(i);
            while at(i).is_some_and(ident_char) {
                i += 1;
            }
            let name_end = offset(i);
            let name = &text[name_start..name_end];
            let (line, col) = byte_to_line_col(&line_starts, text, token_start);
            tokens.push(RustIdent {
                name: name.to_string(),
                line,
                col,
            });
        } else if ident_start(c) {
            // Normal identifier: foo
            let token_start = offset(i);
            while at(i).is_some_and(ident_char) {
                i += 1;
            }
            let token_end = offset(i);
            let name = &text[token_start..token_end];
            let (line, col) = byte_to_line_col(&line_starts, text, token_start);
            tokens.push(RustIdent {
                name: name.to_string(),
                line,
                col,
            });
        } else if c.is_ascii_digit() {
            // Number literal: 123, 0x1f, 1.5e3
            while at(i).is_some_and(|ch| ch == '_' || ch.is_ascii_alphanumeric())
                || (at(i) == Some('.') && at(i + 1).is_some_and(|ch| ch.is_ascii_digit()))
            {
                i += 1;
            }
        } else {
            i += 1;
        }
    }

    tokens
}

/// Finds 1-based UTF-16 column numbers where `name` appears in `line` as a whole identifier.
pub fn identifier_columns(line: &str, name: &str) -> Vec<u32> {
    if name.is_empty() {
        return Vec::new();
    }
    let bytes = line.as_bytes();
    let mut cols = Vec::new();
    let mut from = 0;
    while let Some(pos) = line[from..].find(name) {
        let start = from + pos;
        let end = start + name.len();
        let before_ok =
            start == 0 || !(bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_');
        let after_ok =
            end >= bytes.len() || !(bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_');
        if before_ok && after_ok {
            let col = line[..start].chars().map(|c| c.len_utf16()).sum::<usize>() as u32 + 1;
            cols.push(col);
        }
        from = end;
    }
    cols
}

/// Whether `line` mentions `name` as a whole identifier.
pub fn mentions_identifier(line: &str, name: &str) -> bool {
    !identifier_columns(line, name).is_empty()
}
