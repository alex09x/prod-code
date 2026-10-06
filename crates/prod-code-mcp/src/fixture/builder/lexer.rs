/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::KEYWORDS;
use anyhow::{Context, Result, bail};

/// `r#type` -> `type`.
pub(crate) fn bare(name: &str) -> &str {
    name.strip_prefix("r#").unwrap_or(name)
}

pub(crate) fn is_plain_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c == '_' || c.is_alphabetic())
        && chars.all(|c| c == '_' || c.is_alphanumeric())
        && name != "_"
        && !KEYWORDS.contains(&name)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Ident,
    Lifetime,
    Literal,
    Punct,
}

/// A token's kind and its byte range in the text.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Token {
    pub(crate) kind: Kind,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

/// A file's text and its tokens, comments left out.
pub(crate) struct Source<'a> {
    pub(crate) text: &'a str,
    pub(crate) tokens: Vec<Token>,
    pub(crate) line_starts: Vec<usize>,
}

impl<'a> Source<'a> {
    pub(crate) fn new(text: &'a str) -> Result<Self> {
        let line_starts = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        Ok(Self {
            text,
            tokens: lex(text)?,
            line_starts,
        })
    }

    pub(crate) fn t(&self, i: usize) -> &'a str {
        let token = self.tokens[i];
        &self.text[token.start..token.end]
    }

    pub(crate) fn is(&self, i: usize, text: &str) -> bool {
        i < self.tokens.len() && self.tokens[i].kind != Kind::Literal && self.t(i) == text
    }

    /// The 1-based line token `i` starts on.
    pub(crate) fn line(&self, i: usize) -> u32 {
        self.line_starts
            .partition_point(|&s| s <= self.tokens[i].start) as u32
    }

    /// The closing bracket of the one opened at token `open`.
    pub(crate) fn close_of(&self, open: usize) -> Option<usize> {
        let mut depth = 0i32;
        for i in open..self.tokens.len() {
            if self.tokens[i].kind != Kind::Punct {
                continue;
            }
            match self.t(i) {
                "(" | "[" | "{" => depth += 1,
                ")" | "]" | "}" => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// The opening bracket of the one closed at token `close`.
    pub(crate) fn open_of(&self, close: usize) -> Option<usize> {
        let mut depth = 0i32;
        for i in (0..=close).rev() {
            if self.tokens[i].kind != Kind::Punct {
                continue;
            }
            match self.t(i) {
                ")" | "]" | "}" => depth += 1,
                "(" | "[" | "{" => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// `serde`, `rustfmt::skip`: the path an attribute starts with, from token `from`.
    pub(crate) fn attribute_path(&self, from: usize, to: usize) -> String {
        let mut path = String::new();
        for i in from..to {
            match self.tokens[i].kind {
                Kind::Ident => path.push_str(self.t(i)),
                Kind::Punct if self.t(i) == "::" => path.push_str("::"),
                _ => break,
            }
        }
        path
    }

    /// Tokens `from..to` as written, with every gap between two of them (whitespace, comments,
    /// line breaks) collapsed to one space.
    pub(crate) fn spell(&self, from: usize, to: usize) -> String {
        let mut out = String::new();
        for i in from..to {
            if i > from && self.tokens[i].start > self.tokens[i - 1].end {
                out.push(' ');
            }
            out.push_str(self.t(i));
        }
        out
    }
}

/// Splits Rust source into identifiers, lifetimes, literals and punctuation, dropping
/// whitespace and comments. `::`, `->` and `=>` are single tokens, so a `>` is always a bracket.
pub(crate) fn lex(text: &str) -> Result<Vec<Token>> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let at = |i: usize| chars.get(i).map(|&(_, c)| c);
    let offset = |i: usize| chars.get(i).map_or(text.len(), |&(o, _)| o);
    let ident_start = |c: char| c == '_' || c.is_alphabetic();
    let ident_char = |c: char| c == '_' || c.is_alphanumeric();
    let mut tokens = Vec::new();
    let mut i = 0;
    while let Some(c) = at(i) {
        let start = i;
        let mut push = |kind: Kind, end: usize| {
            tokens.push(Token {
                kind,
                start: offset(start),
                end: offset(end),
            })
        };
        if c.is_whitespace() {
            i += 1;
        } else if c == '/' && at(i + 1) == Some('/') {
            while at(i).is_some_and(|c| c != '\n') {
                i += 1;
            }
        } else if c == '/' && at(i + 1) == Some('*') {
            let mut depth = 0usize;
            loop {
                match (at(i), at(i + 1)) {
                    (Some('/'), Some('*')) => {
                        depth += 1;
                        i += 2;
                    }
                    (Some('*'), Some('/')) => {
                        depth -= 1;
                        i += 2;
                        if depth == 0 {
                            break;
                        }
                    }
                    (Some(_), _) => i += 1,
                    (None, _) => bail!("unterminated block comment at byte {}", offset(start)),
                }
            }
        } else if c == 'r' && at(i + 1) == Some('#') && at(i + 2).is_some_and(ident_start) {
            i += 2;
            while at(i).is_some_and(ident_char) {
                i += 1;
            }
            push(Kind::Ident, i);
        } else if let Some(end) = quoted_end(&chars, i)
            .with_context(|| format!("cannot read the literal at byte {}", offset(start)))?
        {
            i = end;
            push(Kind::Literal, i);
        } else if ident_start(c) {
            while at(i).is_some_and(ident_char) {
                i += 1;
            }
            push(Kind::Ident, i);
        } else if c.is_ascii_digit() {
            while at(i).is_some_and(|c| c == '_' || c.is_ascii_alphanumeric())
                || (at(i) == Some('.') && at(i + 1).is_some_and(|c| c.is_ascii_digit()))
            {
                i += 1;
            }
            push(Kind::Literal, i);
        } else if c == '\'' {
            if at(i + 1) == Some('\\') {
                let mut j = i + 3;
                while at(j).is_some_and(|c| c != '\'') {
                    j += 1;
                }
                if at(j).is_none() {
                    bail!("unterminated character literal at byte {}", offset(start));
                }
                i = j + 1;
                push(Kind::Literal, i);
            } else if at(i + 1).is_some() && at(i + 2) == Some('\'') {
                i += 3;
                push(Kind::Literal, i);
            } else if at(i + 1).is_some_and(ident_start) {
                i += 1;
                if at(i) == Some('r') && at(i + 1) == Some('#') {
                    i += 2;
                }
                while at(i).is_some_and(ident_char) {
                    i += 1;
                }
                push(Kind::Lifetime, i);
            } else {
                bail!("cannot read the `'` at byte {}", offset(start));
            }
        } else {
            let pair = match (c, at(i + 1)) {
                (':', Some(':')) | ('-', Some('>')) | ('=', Some('>')) => true,
                _ => false,
            };
            if pair {
                i += 2;
            } else {
                i += 1;
            }
            push(Kind::Punct, i);
        }
    }
    Ok(tokens)
}

fn quoted_end(chars: &[(usize, char)], i: usize) -> Result<Option<usize>> {
    let at = |i: usize| chars.get(i).map(|&(_, c)| c);
    let mut j = i;
    if matches!(at(j), Some('b') | Some('c')) {
        j += 1;
    }
    if at(j) == Some('r') && matches!(at(j + 1), Some('"') | Some('#')) {
        j += 1;
        let mut hashes = 0;
        while at(j) == Some('#') {
            hashes += 1;
            j += 1;
        }
        if at(j) != Some('"') {
            return Ok(None);
        }
        j += 1;
        loop {
            match at(j) {
                None => bail!("unterminated raw string"),
                Some('"') if (1..=hashes).all(|k| at(j + k) == Some('#')) => {
                    return Ok(Some(j + 1 + hashes));
                }
                _ => j += 1,
            }
        }
    }
    let quote = match at(j) {
        Some('"') => '"',
        Some('\'') if j == i + 1 && at(i) == Some('b') => '\'',
        _ => return Ok(None),
    };
    j += 1;
    loop {
        match at(j) {
            None => bail!("unterminated literal"),
            Some('\\') => j += 2,
            Some(c) if c == quote => return Ok(Some(j + 1)),
            _ => j += 1,
        }
    }
}
