/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

/// Swift strings contain executable interpolation, including nested strings and
/// raw/multiline delimiters. Only the analyzer supplies field identity; this scan
/// determines whether that exact byte belongs to executable code or literal text.
pub(super) fn reference_is_code(text: &str, at: usize) -> bool {
    let mut code = false;
    scan_code(text, 0, at, false, &mut code);
    code
}

fn scan_code(text: &str, mut i: usize, at: usize, interpolation: bool, code: &mut bool) -> usize {
    let mut depth = 0usize;
    while i < text.len() {
        let rest = &text[i..];
        if rest.starts_with("//") {
            i += rest.find('\n').unwrap_or(rest.len());
        } else if rest.starts_with("/*") {
            i += 2;
            let mut comments = 1;
            while i < text.len() && comments > 0 {
                if text[i..].starts_with("/*") {
                    comments += 1;
                    i += 2;
                } else if text[i..].starts_with("*/") {
                    comments -= 1;
                    i += 2;
                } else {
                    i += text[i..].chars().next().unwrap().len_utf8();
                }
            }
        } else if let Some((hashes, quotes)) = string_start(rest) {
            i = scan_string(text, i, hashes, quotes, at, code);
        } else {
            if i == at {
                *code = true;
            }
            match text.as_bytes()[i] {
                b'(' => depth += 1,
                b')' if depth == 0 && interpolation => return i + 1,
                b')' => depth = depth.saturating_sub(1),
                _ => {}
            }
            i += rest.chars().next().unwrap().len_utf8();
        }
    }
    i
}

fn string_start(text: &str) -> Option<(usize, usize)> {
    let hashes = text.bytes().take_while(|b| *b == b'#').count();
    let rest = &text[hashes..];
    if rest.starts_with("\"\"\"") {
        Some((hashes, 3))
    } else if rest.starts_with('"') {
        Some((hashes, 1))
    } else {
        None
    }
}

fn scan_string(
    text: &str,
    start: usize,
    hashes: usize,
    quotes: usize,
    at: usize,
    code: &mut bool,
) -> usize {
    let closing = format!("{}{}", "\"".repeat(quotes), "#".repeat(hashes));
    let escape = format!("\\{}", "#".repeat(hashes));
    let marker = format!("{escape}(");
    let mut i = start + hashes + quotes;
    while i < text.len() {
        let rest = &text[i..];
        if rest.starts_with(&closing) {
            return i + closing.len();
        }
        if rest.starts_with(&marker) {
            i = scan_code(text, i + marker.len(), at, true, code);
        } else if rest.starts_with(&escape) {
            i += escape.len();
            if i < text.len() {
                i += text[i..].chars().next().unwrap().len_utf8();
            }
        } else {
            i += rest.chars().next().unwrap().len_utf8();
        }
    }
    i
}
