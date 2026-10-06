/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;

pub fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

pub fn is_ident_str(s: &str) -> bool {
    !s.is_empty() && s.chars().all(is_ident)
}

pub fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

pub fn reindent(text: &str, indent: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return String::new();
    }
    let min_indent = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    lines
        .iter()
        .map(|l| {
            if l.trim().is_empty() {
                String::new()
            } else {
                let stripped = if l.len() >= min_indent {
                    &l[min_indent..]
                } else {
                    l.trim_start()
                };
                format!("{indent}{stripped}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Top-level pieces of `text` separated by `sep`, as byte ranges, outside every bracket.
pub fn split_top(text: &str, sep: char) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    let mut prev = ' ';
    for (i, c) in text.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            '<' => depth += 1,
            '>' if prev != '-' && prev != '=' => depth -= 1,
            c if c == sep && depth == 0 => {
                out.push((start, i));
                start = i + c.len_utf8();
            }
            _ => {}
        }
        prev = c;
    }
    if !text[start..].trim().is_empty() {
        out.push((start, text.len()));
    }
    out
}
