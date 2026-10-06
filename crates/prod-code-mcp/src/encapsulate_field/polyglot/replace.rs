/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::case::is_ident;

pub fn replace_line_this(line: &str, field: &str) -> (String, usize) {
    let mut out = String::new();
    let mut count = 0;
    let mut rest = line;
    let needle = format!("this.{field}");
    while let Some(pos) = rest.find(&needle) {
        let after_pos = pos + needle.len();
        let after_char = rest[after_pos..].chars().next();
        if after_char.is_none_or(|c| !c.is_alphanumeric() && c != '_') {
            out.push_str(&rest[..pos]);
            out.push_str(&format!("this._{field}"));
            count += 1;
            rest = &rest[after_pos..];
        } else {
            out.push_str(&rest[..after_pos]);
            rest = &rest[after_pos..];
        }
    }
    out.push_str(rest);
    (out, count)
}

pub fn replace_line_this_private(line: &str, field: &str) -> (String, usize) {
    let mut out = String::new();
    let mut count = 0;
    let mut rest = line;
    let needle = format!("this.{field}");
    while let Some(pos) = rest.find(&needle) {
        let after_pos = pos + needle.len();
        let after_char = rest[after_pos..].chars().next();
        if after_char.is_none_or(|c| !c.is_alphanumeric() && c != '_') {
            out.push_str(&rest[..pos]);
            out.push_str(&format!("this.#{field}"));
            count += 1;
            rest = &rest[after_pos..];
        } else {
            out.push_str(&rest[..after_pos]);
            rest = &rest[after_pos..];
        }
    }
    out.push_str(rest);
    (out, count)
}

pub fn replace_line_self(line: &str, field: &str) -> (String, usize) {
    let mut out = String::new();
    let mut count = 0;
    let mut rest = line;
    let needle = format!("self.{field}");
    while let Some(pos) = rest.find(&needle) {
        let after_pos = pos + needle.len();
        let after_char = rest[after_pos..].chars().next();
        if after_char.is_none_or(|c| !c.is_alphanumeric() && c != '_') {
            out.push_str(&rest[..pos]);
            out.push_str(&format!("self._{field}"));
            count += 1;
            rest = &rest[after_pos..];
        } else {
            out.push_str(&rest[..after_pos]);
            rest = &rest[after_pos..];
        }
    }
    out.push_str(rest);
    (out, count)
}

pub fn replace_cpp_unqualified(
    line: &str,
    field: &str,
    in_block_comment: &mut bool,
) -> (String, usize, bool) {
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    let mut out = String::with_capacity(line.len());
    let mut changed = 0;
    let mut shadowed = false;
    let mut i = 0;
    let mut quote = None;
    let mut escaped = false;
    while i < chars.len() {
        let (at, ch) = chars[i];
        let next = chars.get(i + 1).map(|(_, c)| *c);
        if *in_block_comment {
            out.push(ch);
            if ch == '*' && next == Some('/') {
                out.push('/');
                i += 2;
                *in_block_comment = false;
            } else {
                i += 1;
            }
            continue;
        }
        if let Some(delimiter) = quote {
            out.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == delimiter {
                quote = None;
            }
            i += 1;
            continue;
        }
        if ch == '/' && next == Some('/') {
            out.push_str(&line[at..]);
            break;
        }
        if ch == '/' && next == Some('*') {
            out.push_str("/*");
            i += 2;
            *in_block_comment = true;
            continue;
        }
        if ch == '"' || ch == '\'' {
            quote = Some(ch);
            out.push(ch);
            i += 1;
            continue;
        }
        if is_ident(ch) && !ch.is_ascii_digit() {
            let start_i = i;
            i += 1;
            while i < chars.len() && is_ident(chars[i].1) {
                i += 1;
            }
            let start = chars[start_i].0;
            let end = chars.get(i).map_or(line.len(), |(byte, _)| *byte);
            let before = line[..start].trim_end();
            let after = line[end..].trim_start();
            let qualified_this = before.ends_with("this->");
            let qualified = (before.ends_with('.') || before.ends_with('>')) && !qualified_this;
            if &line[start..end] == field
                && (!qualified || qualified_this)
                && !after.starts_with('(')
            {
                let previous = before.split_whitespace().next_back().unwrap_or_default();
                if matches!(after.chars().next(), Some('=' | ';' | ',' | ')' | '{'))
                    && !matches!(previous, "return" | "throw" | "co_return" | "case")
                    && !before.ends_with('(')
                {
                    shadowed = true;
                }
                out.push_str(&format!("{field}_"));
                changed += 1;
            } else {
                out.push_str(&line[start..end]);
            }
            continue;
        }
        out.push(ch);
        i += 1;
    }
    (out, changed, shadowed)
}
