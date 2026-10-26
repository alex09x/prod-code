/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::parameter_object::{Language, syntax::walk_code};
use anyhow::{Context, Result, ensure};
use std::sync::LazyLock;

/// Consume a complete result annotation before looking for a method body. The
/// language walker skips comments; the local literal scanner also keeps nested
/// template types opaque. Balanced type interiors cannot open the body.
pub(super) fn body_open(text: &str, from: usize) -> Result<usize> {
    let mut parser = ResultType {
        text,
        spans: spans(text, from),
        at: 0,
    };
    if parser.eat(":") {
        parser.ty(0)?;
    }
    ensure!(
        parser.peek() == "{",
        "unsupported TypeScript result type or missing method body"
    );
    Ok(parser.spans[parser.at].0)
}

pub(super) fn spans(text: &str, mut from: usize) -> Vec<(usize, usize)> {
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let view = super::ts_mask::line_view(text);
    while from < text.len() {
        let mut literal = None;
        let mut token_end = from;
        walk_code(&view, from, Language::TypeScript, |at, byte| {
            if at < token_end {
                return true;
            }
            if (identifier_byte(byte) && !byte.is_ascii_digit())
                || unicode_escape(text, at).is_some()
            {
                token_end = identifier_end(text, at);
                spans.push((at, token_end));
            } else if byte.is_ascii_digit()
                || (byte == b'.' && text.as_bytes().get(at + 1).is_some_and(u8::is_ascii_digit))
            {
                token_end = number_end(text.as_bytes(), at);
                spans.push((at, token_end));
            } else if matches!(byte, b'\'' | b'"' | b'`') {
                literal = Some(at);
                return false;
            } else if !byte.is_ascii_whitespace() {
                spans.push((at, at + 1));
            }
            true
        });
        let Some(start) = literal else { break };
        // Restart the walker after the complete literal, so its flat quote
        // state cannot leak out of a nested template. An incomplete literal
        // consumes the tail and cannot manufacture a method body delimiter.
        from = super::ts_literal::end(text, start).unwrap_or(text.len());
        spans.push((start, from));
    }
    spans
}

/// Bound a nested signature's result using the same type grammar as body_open.
/// Conditional types can occur inside its opaque object/tuple groups too.
pub(super) fn type_end(text: &str, from: usize) -> Option<usize> {
    let mut parser = ResultType {
        text,
        spans: spans(text, from),
        at: 0,
    };
    parser.scoped_type(0).ok()?;
    parser
        .spans
        .get(parser.at.checked_sub(1)?)
        .map(|span| span.1)
}

struct ResultType<'a> {
    text: &'a str,
    spans: Vec<(usize, usize)>,
    at: usize,
}

fn identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$') || byte >= 128
}

pub(super) fn identifier_continue(ch: char) -> bool {
    static CONTINUE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\A[$\u{200C}\u{200D}\p{ID_Continue}]\z").unwrap());
    CONTINUE.is_match(ch.encode_utf8(&mut [0; 4]))
}

pub(super) fn unicode_escape(text: &str, at: usize) -> Option<(usize, char)> {
    let tail = text.get(at..)?.strip_prefix("\\u")?;
    let (digits, end) = if let Some(tail) = tail.strip_prefix('{') {
        let close = tail.find('}')?;
        (tail.get(..close)?, at + 4 + close)
    } else {
        (tail.get(..4)?, at + 6)
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let ch = char::from_u32(u32::from_str_radix(digits, 16).ok()?)?;
    // Identifier escapes can encode continuation-only characters too, such as
    // combining marks. Keep their identity consistent with literal UTF-8 bytes.
    identifier_continue(ch).then_some((end, ch))
}

fn identifier_end(text: &str, mut at: usize) -> usize {
    while at < text.len() {
        if identifier_byte(text.as_bytes()[at]) {
            at += 1;
        } else if let Some((end, _)) = unicode_escape(text, at) {
            at = end;
        } else {
            break;
        }
    }
    at
}

fn identifier(token: &str) -> bool {
    !token.is_empty()
        && !token.as_bytes()[0].is_ascii_digit()
        && identifier_end(token, 0) == token.len()
}

/// Normalize only the guard's view; keep original annotation bytes in output.
/// Escaping a predicate's subject must not bypass the removed-receiver guard.
pub(super) fn identifier_view(text: &str) -> String {
    let mut result = String::new();
    let mut at = 0;
    while at < text.len() {
        if let Some((end, ch)) = unicode_escape(text, at) {
            result.push(ch);
            at = end;
        } else {
            let ch = text[at..].chars().next().expect("character boundary");
            result.push(ch);
            at += ch.len_utf8();
        }
    }
    result
}

/// Keep the dot and exponent sign inside a numeric literal, rather than
/// interpreting them as a qualified name or a separate result-type operator.
fn number_end(bytes: &[u8], mut at: usize) -> usize {
    if bytes.get(at) == Some(&b'0')
        && bytes
            .get(at + 1)
            .is_some_and(|b| matches!(b, b'x' | b'X' | b'o' | b'O' | b'b' | b'B'))
    {
        at += 2;
        while bytes
            .get(at)
            .is_some_and(|b| b.is_ascii_hexdigit() || *b == b'_')
        {
            at += 1;
        }
    } else {
        while bytes
            .get(at)
            .is_some_and(|b| b.is_ascii_digit() || *b == b'_')
        {
            at += 1;
        }
        if bytes.get(at) == Some(&b'.') {
            at += 1;
            while bytes
                .get(at)
                .is_some_and(|b| b.is_ascii_digit() || *b == b'_')
            {
                at += 1;
            }
        }
        if bytes.get(at).is_some_and(|b| matches!(b, b'e' | b'E')) {
            at += 1;
            if bytes.get(at).is_some_and(|b| matches!(b, b'+' | b'-')) {
                at += 1;
            }
            while bytes
                .get(at)
                .is_some_and(|b| b.is_ascii_digit() || *b == b'_')
            {
                at += 1;
            }
        }
    }
    at + usize::from(bytes.get(at) == Some(&b'n'))
}

impl ResultType<'_> {
    fn scoped_type(&mut self, depth: usize) -> Result<()> {
        self.ty(depth)?;
        if self.eat("extends") {
            self.ty(depth + 1)?;
            ensure!(self.eat("?"), "unsupported conditional query scope");
            self.scoped_type(depth + 1)?;
            ensure!(self.eat(":"), "unsupported conditional query scope");
            self.scoped_type(depth + 1)?;
        }
        Ok(())
    }

    fn peek(&self) -> &str {
        self.spans
            .get(self.at)
            .map_or("", |&(start, end)| &self.text[start..end])
    }

    fn eat(&mut self, token: &str) -> bool {
        if self.peek() != token {
            return false;
        }
        self.at += 1;
        true
    }

    fn ty(&mut self, depth: usize) -> Result<()> {
        ensure!(
            depth < 64,
            "unsupported deeply nested TypeScript result type"
        );
        // `asserts` is contextual: it can also name an ordinary result type
        // or namespace. Only an immediately following identifier makes an
        // assertion signature.
        if self.peek() == "asserts"
            && self
                .spans
                .get(self.at + 1)
                .is_some_and(|&(start, end)| identifier(&self.text[start..end]))
        {
            self.at += 2;
            if self.eat("is") {
                self.ty(depth + 1)?;
            }
            return Ok(());
        }
        self.eat("|");
        self.eat("&");
        self.atom(depth + 1)?;
        if self.eat("is") {
            return self.ty(depth + 1);
        }
        while matches!(self.peek(), "|" | "&") {
            self.at += 1;
            self.atom(depth + 1)?;
        }
        Ok(())
    }

    fn atom(&mut self, depth: usize) -> Result<()> {
        ensure!(
            depth < 64,
            "unsupported deeply nested TypeScript result type"
        );
        if matches!(
            self.peek(),
            "keyof" | "typeof" | "readonly" | "unique" | "new"
        ) || (self.peek() == "abstract"
            && self
                .spans
                .get(self.at + 1)
                .is_some_and(|&(start, end)| &self.text[start..end] == "new"))
        {
            // `abstract` is also a valid type/namespace name. Its modifier
            // role is limited to constructor types beginning with `new`.
            self.at += 1;
            return self.atom(depth + 1);
        }
        match self.peek() {
            "(" | "[" | "{" => self.group()?,
            "<" => {
                self.group()?;
                ensure!(
                    self.peek() == "(",
                    "unsupported generic TypeScript callable result"
                );
                self.group()?;
            }
            "-" => {
                self.at += 1;
                ensure!(
                    self.peek()
                        .as_bytes()
                        .first()
                        .is_some_and(|b| b.is_ascii_digit() || *b == b'.'),
                    "unsupported TypeScript literal result"
                );
                self.at += 1;
            }
            token => {
                ensure!(
                    identifier(token)
                        || token
                            .chars()
                            .next()
                            .is_some_and(|ch| ch.is_alphanumeric()
                                || matches!(ch, '_' | '$' | '\'' | '"' | '`'))
                        || (token.starts_with('.')
                            && token.as_bytes().get(1).is_some_and(u8::is_ascii_digit)),
                    "unsupported TypeScript result type"
                );
                let import = token == "import";
                self.at += 1;
                if import && self.peek() == "(" {
                    self.group()?;
                }
            }
        }
        loop {
            match self.peek() {
                "." => {
                    self.at += 1;
                    ensure!(
                        identifier(self.peek()),
                        "unsupported qualified TypeScript result"
                    );
                    self.at += 1;
                }
                "[" | "<" => self.group()?,
                "=" => {
                    self.at += 1;
                    ensure!(self.eat(">"), "unsupported TypeScript callable result");
                    self.ty(depth)?;
                    break;
                }
                _ => break,
            }
        }
        Ok(())
    }

    fn group(&mut self) -> Result<()> {
        let mut stack = Vec::new();
        loop {
            let token = self.peek();
            match token {
                "(" => stack.push(")"),
                "[" => stack.push("]"),
                "{" => stack.push("}"),
                "<" => stack.push(">"),
                ")" | "]" | "}" | ">" => {
                    // A function-type arrow is not a generic delimiter.
                    let arrow = token == ">" && self.at > 0 && {
                        let (start, end) = self.spans[self.at - 1];
                        &self.text[start..end] == "="
                    };
                    if !arrow {
                        ensure!(
                            stack.pop() == Some(token),
                            "unsupported TypeScript result delimiter"
                        );
                    }
                }
                "" => anyhow::bail!("unsupported unterminated TypeScript result type"),
                _ => {}
            }
            self.at += 1;
            if stack.is_empty() {
                return Ok(());
            }
            self.spans
                .get(self.at)
                .context("unsupported unterminated TypeScript result type")?;
        }
    }
}
