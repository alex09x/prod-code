/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

/// Attribute attachment follows Rust token groups, not lines. Comments and literals are
/// opaque, and an item's attributes stop at its body/semicolon. Enclosing attributed
/// modules/impls conservatively protect their declarations too.
pub(crate) struct RustAttributes<'a> {
    text: &'a str,
    tokens: Vec<(usize, usize)>,
}

impl<'a> RustAttributes<'a> {
    pub(crate) fn new(text: &'a str) -> Self {
        let code = crate::extract_trait::lex::lexical_code(text);
        let mut tokens: Vec<(usize, usize)> = Vec::new();
        for (at, ch) in text.char_indices() {
            if !code[at] || ch.is_whitespace() {
                continue;
            }
            if (ch.is_alphanumeric() || ch == '_')
                && let Some((start, end)) = tokens.last_mut()
                && *end == at
                && text[*start..*end]
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '_')
            {
                *end += ch.len_utf8();
            } else {
                tokens.push((at, at + ch.len_utf8()));
            }
        }
        Self { text, tokens }
    }

    /// `None` means the candidate has no attribute with unknown reachability effects.
    /// Coordinates are the analyzer's one-based UTF-16 selection position.
    pub(crate) fn retention_reason(&self, line: u32, col: u32) -> Option<String> {
        let unknown = || Some("Rust attribute attachment could not be established".to_owned());
        let Some(line_text) = self
            .text
            .split_inclusive('\n')
            .nth(line.saturating_sub(1) as usize)
        else {
            return unknown();
        };
        let line_start: usize = self
            .text
            .split_inclusive('\n')
            .take(line.saturating_sub(1) as usize)
            .map(str::len)
            .sum();
        let mut units = 0;
        let Some((byte, _)) = line_text.char_indices().find(|(_, c)| {
            let selected = units == col.saturating_sub(1);
            units += c.len_utf16() as u32;
            selected
        }) else {
            return unknown();
        };
        let target = line_start + byte;
        if !self.tokens.iter().any(|(start, _)| *start == target) {
            return unknown();
        }
        let token = |i: usize| &self.text[self.tokens[i].0..self.tokens[i].1];
        let mut pending = None;
        let mut scopes: Vec<Option<String>> = vec![None];
        let mut i = 0;
        while i < self.tokens.len() && self.tokens[i].0 < target {
            match token(i) {
                "#" => {
                    let inner = i + 1 < self.tokens.len() && token(i + 1) == "!";
                    let open = i + if inner { 2 } else { 1 };
                    if open >= self.tokens.len() || token(open) != "[" {
                        return unknown();
                    }
                    let mut delimiters = vec!["]"];
                    let mut end = open + 1;
                    while end < self.tokens.len() && !delimiters.is_empty() {
                        match token(end) {
                            "[" => delimiters.push("]"),
                            "(" => delimiters.push(")"),
                            "{" => delimiters.push("}"),
                            "]" | ")" | "}" if delimiters.pop() != Some(token(end)) => {
                                return unknown();
                            }
                            _ => {}
                        }
                        end += 1;
                    }
                    if !delimiters.is_empty() || self.tokens[end - 1].0 >= target {
                        return unknown();
                    }
                    let name = token(open + 1);
                    let simple = !matches!(token(open + 2), ":");
                    // These built-ins cannot register a declaration or expose an ABI symbol.
                    // All other attributes, including cfg_attr and proc macros, need proof
                    // beyond reference absence and are deliberately retained.
                    if !simple
                        || !matches!(
                            name,
                            "allow"
                                | "warn"
                                | "deny"
                                | "forbid"
                                | "expect"
                                | "inline"
                                | "cold"
                                | "must_use"
                                | "deprecated"
                                | "doc"
                                | "repr"
                                | "non_exhaustive"
                        )
                    {
                        let reason = Some(format!(
                            "Rust attribute `{name}` may expose or register this declaration"
                        ));
                        if inner {
                            *scopes.last_mut().unwrap() = reason;
                        } else {
                            pending = reason;
                        }
                    }
                    i = end;
                    continue;
                }
                "{" => {
                    // A completed sibling body cannot attach attributes to this item.
                    // This also keeps macro token trees in that body out of the scan.
                    let mut depth = 1;
                    let mut close = i + 1;
                    while close < self.tokens.len() && depth > 0 {
                        match token(close) {
                            "{" => depth += 1,
                            "}" => depth -= 1,
                            _ => {}
                        }
                        if depth > 0 {
                            close += 1;
                        }
                    }
                    if depth == 0 && self.tokens[close].0 < target {
                        pending = None;
                        i = close + 1;
                        continue;
                    }
                    scopes.push(pending.take().or_else(|| scopes.last().cloned().flatten()));
                }
                "}" => {
                    if scopes.len() == 1 {
                        return unknown();
                    }
                    scopes.pop();
                    pending = None;
                }
                ";" => pending = None,
                _ => {}
            }
            i += 1;
        }
        pending.or_else(|| scopes.last().cloned().flatten())
    }
}
