/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

/// Strips ANSI CSI and OSC escape sequences from `s`.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.peek() == Some(&'[') {
                chars.next();
                while let Some(&next) = chars.peek() {
                    chars.next();
                    if (0x40..=0x7E).contains(&(next as u32)) {
                        break;
                    }
                }
            } else if chars.peek() == Some(&']') {
                chars.next();
                while let Some(&next) = chars.peek() {
                    chars.next();
                    if next == '\x07' {
                        break;
                    }
                    if next == '\x1b' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The raw lines `from..=to`, escape sequences included.
pub(crate) fn raw_excerpt(raw: &[&str], from: usize, to: usize) -> String {
    raw[from..=to].join("\n")
}

pub(crate) fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// Whether every double-quoted string and every bracket in a printed value closes. With
/// `rust_chars`, Rust `Debug` char literals (`'{'`, `'"'`, `'\''`) are skipped as well; any other
/// apostrophe is text.
pub(crate) fn closes(value: &str, rust_chars: bool) -> bool {
    let mut open = Vec::new();
    let mut in_string = false;
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if in_string {
            match c {
                '\\' => {
                    chars.next();
                }
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            '\'' if rust_chars => {
                let mut ahead = chars.clone();
                let literal = match ahead.next() {
                    Some('\\') => ahead.next().is_some() && ahead.any(|c| c == '\''),
                    Some(_) => ahead.next() == Some('\''),
                    None => false,
                };
                if literal {
                    chars = ahead;
                }
            }
            '"' => in_string = true,
            '(' | '[' | '{' => open.push(c),
            ')' | ']' | '}' => {
                let want = match c {
                    ')' => '(',
                    ']' => '[',
                    _ => '{',
                };
                if open.pop() != Some(want) {
                    return false;
                }
            }
            _ => {}
        }
    }
    !in_string && open.is_empty()
}
