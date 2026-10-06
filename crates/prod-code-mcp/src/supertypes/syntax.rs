/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

/// The identifier around the 1-based character `col` of `line`.
pub(crate) fn word_at(line: &str, col: u32) -> Option<String> {
    let chars: Vec<char> = line.chars().collect();
    let at = (col as usize).checked_sub(1)?;
    let is_word = |c: &char| c.is_alphanumeric() || *c == '_';
    if !chars.get(at).is_some_and(is_word) {
        return None;
    }
    let start = (0..=at).rev().take_while(|i| is_word(&chars[*i])).last()?;
    let end = (at..chars.len())
        .take_while(|i| is_word(&chars[*i]))
        .last()?
        + 1;
    Some(chars[start..end].iter().collect())
}

/// The text of `lines` from `from` (0-based) up to the first `{` or `;`, on one line.
pub(crate) fn header_from(lines: &[&str], from: usize) -> String {
    let mut header = String::new();
    for line in lines.iter().skip(from) {
        match line.find(['{', ';']) {
            Some(end) => {
                header.push_str(&line[..end]);
                break;
            }
            None => {
                header.push_str(line);
                header.push(' ');
            }
        }
    }
    header
}

/// `text` split at `sep` where it is outside `<…>` and `(…)`.
pub(crate) fn split_top(text: &str, sep: char) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut current = String::new();
    for c in text.chars() {
        match c {
            '<' | '(' => depth += 1,
            '>' | ')' => depth -= 1,
            _ => {}
        }
        if c == sep && depth == 0 {
            parts.push(current.trim().to_string());
            current.clear();
        } else {
            current.push(c);
        }
    }
    parts.push(current.trim().to_string());
    parts.into_iter().filter(|p| !p.is_empty()).collect()
}

/// `text` after a leading `<…>`, with the brackets balanced.
pub(crate) fn skip_generics(text: &str) -> &str {
    let text = text.trim_start();
    if !text.starts_with('<') {
        return text;
    }
    let mut depth = 0;
    for (i, c) in text.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => {
                depth -= 1;
                if depth == 0 {
                    return &text[i + 1..];
                }
            }
            _ => {}
        }
    }
    ""
}

/// The trait an impl header implements: `Default` for `impl Default for Cache`, `Into<u8>` for
/// `impl<T> Into<u8> for Wrapper<T>`; `None` for an inherent `impl Cache`.
pub fn impl_trait(header: &str) -> Option<String> {
    let at = header.find("impl")?;
    let rest = skip_generics(&header[at + 4..]);
    let rest = rest.split(" where ").next().unwrap_or(rest);
    let mut depth = 0i32;
    let chars: Vec<(usize, char)> = rest.char_indices().collect();
    for (n, &(i, c)) in chars.iter().enumerate() {
        match c {
            '<' | '(' => depth += 1,
            '>' | ')' => depth -= 1,
            _ => {}
        }
        if depth == 0 && rest[i..].starts_with(" for ") && n > 0 {
            let name = rest[..i].trim();
            return (!name.is_empty()).then(|| name.to_string());
        }
    }
    None
}

/// The supertraits in a trait header: `Send + Sync` in `pub trait Embed: Send + Sync {`, and
/// the bounds on `Self` in its `where` clause, which the Rust Reference counts as supertraits
/// too (`trait Circle where Self: Shape`, #227).
pub fn supertraits(header: &str) -> Vec<String> {
    let Some(at) = header.find("trait ") else {
        return Vec::new();
    };
    let after = &header[at + 6..];
    let name_end = after
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(after.len());
    let rest = skip_generics(&after[name_end..]).trim_start();
    // `where` as a word, not inside a name such as `Somewhere`.
    let keyword = rest.match_indices("where").map(|(i, _)| i).find(|&i| {
        (i == 0 || rest[..i].ends_with(char::is_whitespace))
            && rest[i + 5..].starts_with(char::is_whitespace)
    });
    let (bounds, clause) = match keyword {
        Some(w) => (&rest[..w], &rest[w + "where".len()..]),
        None => (rest, ""),
    };
    let mut out = bounds
        .trim_start()
        .strip_prefix(':')
        .map(|b| split_top(b, '+'))
        .unwrap_or_default();
    for predicate in split_top(clause, ',') {
        if let Some(on_self) = predicate.strip_prefix("Self")
            && let Some(b) = on_self.trim_start().strip_prefix(':')
        {
            for bound in split_top(b, '+') {
                if !out.contains(&bound) {
                    out.push(bound);
                }
            }
        }
    }
    out
}

/// The traits derived by the `#[derive(…)]` attributes directly above the declaration on line
/// `decl` (0-based), each with its 1-based position. An attribute may span lines.
pub(crate) fn derives_above(lines: &[&str], decl: usize) -> Vec<(String, u32, u32)> {
    if lines.is_empty() || decl == 0 {
        return Vec::new();
    }
    // The attributes and doc comments of this item: up to the end of the one before it.
    let mut start = decl.min(lines.len());
    while start > 0 {
        let above = lines[start - 1].trim();
        if above.is_empty() || above.ends_with('}') || above.ends_with(';') || decl - start >= 40 {
            break;
        }
        start -= 1;
    }
    let mut out = Vec::new();
    let mut inside = false;
    for (n, line) in lines
        .iter()
        .enumerate()
        .take(decl.min(lines.len()))
        .skip(start)
    {
        let mut from = 0;
        if !inside {
            match line.find("derive(") {
                Some(at) if line.trim_start().starts_with("#[") || line[..at].contains("#[") => {
                    inside = true;
                    from = at + "derive(".len();
                }
                _ => continue,
            }
        }
        let body = &line[from..];
        let end = body.find(')');
        let names = &body[..end.unwrap_or(body.len())];
        let mut offset = from;
        for part in names.split(',') {
            let name = part.trim();
            if !name.is_empty() {
                let col = line[..offset + part.find(name).unwrap_or(0)]
                    .chars()
                    .count() as u32
                    + 1;
                out.push((name.to_string(), n as u32 + 1, col));
            }
            offset += part.len() + 1;
        }
        if end.is_some() {
            inside = false;
        }
    }
    out
}

/// The line an impl header starts on, when the location on line `at` (0-based) is in one: at
/// most three lines up, for a header broken over lines.
pub(crate) fn impl_header_start(lines: &[&str], at: usize) -> Option<usize> {
    (at.saturating_sub(3)..=at).rev().find(|i| {
        let line = lines[*i].trim_start();
        line.starts_with("impl") || line.starts_with("unsafe impl")
    })
}

/// Is the declaration on this line a trait?
pub(crate) fn is_trait_decl(line: &str) -> bool {
    let mut rest = line.trim_start();
    for prefix in ["pub(crate) ", "pub(super) ", "pub ", "unsafe ", "auto "] {
        rest = rest.strip_prefix(prefix).unwrap_or(rest);
    }
    rest.starts_with("trait ")
}
