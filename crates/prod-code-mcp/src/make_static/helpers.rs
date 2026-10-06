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

pub fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

pub fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Whether `word` occurs in `text` as a whole identifier.
pub fn mentions(text: &str, word: &str) -> bool {
    text.match_indices(word).any(|(at, _)| {
        !text[..at].chars().next_back().is_some_and(is_ident)
            && !text[at + word.len()..].chars().next().is_some_and(is_ident)
    })
}

/// The receiver a parameter list starts with, and the list without it, or `None` when the first
/// parameter is not a receiver.
pub fn split_receiver(params: &str) -> Option<(String, String)> {
    let parts = crate::signature::split_params(params);
    let first = parts.first()?.trim().to_string();
    // `self`, `mut self`, `&self`, `&mut self`, `&'a self`, `self: Box<Self>`.
    let head = first.split(':').next().unwrap_or("").trim();
    let mut word = head.trim_start_matches('&').trim_start();
    if word.starts_with('\'') {
        word = word
            .split_once(char::is_whitespace)
            .map_or("", |(_, rest)| rest)
            .trim_start();
    }
    let word = word.strip_prefix("mut ").unwrap_or(word).trim();
    if word != "self" {
        return None;
    }
    let rest: Vec<String> = parts[1..].iter().map(|p| p.trim().to_string()).collect();
    Some((first, rest.join(", ")))
}

/// Whether evaluating `receiver` can do anything: a call, `?`, `.await` or a macro. A path, a
/// field access, `self` or a literal can be dropped without changing what the program does.
pub fn receiver_has_effects(receiver: &str) -> bool {
    let r = receiver.trim();
    r.contains('(') || r.contains('?') || r.contains('!') || r.contains(".await") || r.contains('[')
}

pub fn extract_receiver(before: &str) -> Option<&str> {
    let trimmed = before.trim_end();
    if trimmed.is_empty() {
        return None;
    }
    let bytes = trimmed.as_bytes();
    let mut i = bytes.len();
    let mut depth_paren = 0i32;
    let mut depth_bracket = 0i32;

    while i > 0 {
        let b = bytes[i - 1];
        match b {
            b')' => depth_paren += 1,
            b'(' => {
                if depth_paren > 0 {
                    depth_paren -= 1;
                } else {
                    break;
                }
            }
            b']' => depth_bracket += 1,
            b'[' => {
                if depth_bracket > 0 {
                    depth_bracket -= 1;
                } else {
                    break;
                }
            }
            _ => {
                if depth_paren == 0 && depth_bracket == 0 {
                    let c = b as char;
                    if !(is_ident(c) || c == '.' || c == '?' || c == '!' || c == '>' || c == '-') {
                        break;
                    }
                }
            }
        }
        i -= 1;
    }
    let recv = trimmed[i..].trim_start_matches("return ").trim_start();
    if recv.is_empty() { None } else { Some(recv) }
}

pub fn find_method_at_line(code: &str, line_1based: u32) -> Option<(String, Option<String>)> {
    let lines: Vec<&str> = code.lines().collect();
    if line_1based == 0 || line_1based as usize > lines.len() {
        return None;
    }
    let target_idx = (line_1based - 1) as usize;
    let start_idx = target_idx.saturating_sub(2);
    let end_idx = std::cmp::min(target_idx + 2, lines.len().saturating_sub(1));

    for line in lines.iter().take(end_idx + 1).skip(start_idx) {
        let trimmed = line.trim();
        if trimmed.starts_with("def ") || trimmed.starts_with("async def ") {
            let rest = trimmed.strip_prefix("async ").unwrap_or(trimmed);
            if let Some(after_def) = rest.strip_prefix("def ")
                && let Some(paren) = after_def.find('(')
            {
                let name = after_def[..paren].trim();
                return Some((name.to_string(), None));
            }
        }
        if let Some(after_func) = trimmed.strip_prefix("func ") {
            if after_func.starts_with('(') {
                if let Some(close_recv) = after_func.find(')') {
                    let recv_part = &after_func[1..close_recv];
                    let type_name = recv_part
                        .split_whitespace()
                        .last()
                        .map(|t| t.trim_start_matches('*'))
                        .unwrap_or("");
                    let after_recv = after_func[close_recv + 1..].trim_start();
                    if let Some(paren) = after_recv.find('(') {
                        let name = after_recv[..paren].trim();
                        return Some((name.to_string(), Some(type_name.to_string())));
                    }
                }
            } else if let Some(paren) = after_func.find('(') {
                let name = after_func[..paren].trim();
                return Some((name.to_string(), None));
            }
        }
        if let Some(pos) = trimmed.find("func ") {
            let after = &trimmed[pos + 5..];
            if let Some(paren) = after.find('(') {
                let name = after[..paren].trim();
                return Some((name.to_string(), None));
            }
        }
        if let Some(paren) = trimmed.find('(') {
            let before = trimmed[..paren].trim();
            if let Some(name) = before.split_whitespace().last() {
                let clean = name.trim_start_matches('*').trim_start_matches('&');
                if !clean.is_empty()
                    && clean.chars().all(is_ident)
                    && clean != "if"
                    && clean != "while"
                    && clean != "for"
                    && clean != "switch"
                    && clean != "catch"
                {
                    return Some((clean.to_string(), None));
                }
            }
        }
    }
    None
}
