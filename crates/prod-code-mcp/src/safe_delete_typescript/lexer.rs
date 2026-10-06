/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result};

pub fn matching(text: &str, open: usize) -> Result<usize> {
    let first = *text
        .as_bytes()
        .get(open)
        .context("a delimiter is outside the source")?;
    anyhow::ensure!(
        matches!(first, b'(' | b'[' | b'{'),
        "expected an opening delimiter"
    );
    let mut stack = vec![first];
    let mut cursor = open + 1;
    while cursor < text.len() {
        if let Some(next) = opaque_end(text, cursor)? {
            cursor = next;
            continue;
        }
        let byte = text.as_bytes()[cursor];
        if byte == b'/' {
            anyhow::bail!(
                "regular-expression and division syntax is outside the safe-delete subset"
            );
        }
        match byte {
            b'(' | b'[' | b'{' => stack.push(byte),
            b')' | b']' | b'}' => {
                let opened = stack
                    .pop()
                    .context("an unmatched delimiter closes the declaration")?;
                anyhow::ensure!(
                    pair(opened, byte),
                    "a delimiter is mismatched in the declaration"
                );
                if stack.is_empty() {
                    return Ok(cursor);
                }
            }
            _ => {}
        }
        cursor = advance(text, cursor);
    }
    anyhow::bail!("an opening delimiter in the declaration is not closed")
}

pub fn opaque_end(text: &str, start: usize) -> Result<Option<usize>> {
    let bytes = text.as_bytes();
    let Some(&first) = bytes.get(start) else {
        return Ok(None);
    };
    let end = match first {
        quote @ (b'\'' | b'"') => {
            let mut cursor = start + 1;
            loop {
                let byte = *bytes
                    .get(cursor)
                    .context("a quoted literal is not terminated")?;
                match byte {
                    b'\\' => {
                        let escaped = cursor
                            .checked_add(1)
                            .filter(|escaped| *escaped < text.len())
                            .context("a quoted escape is truncated")?;
                        cursor = advance(text, escaped);
                    }
                    b'\n' | b'\r' => {
                        anyhow::bail!("a quoted literal crosses a line without closing")
                    }
                    value if value == quote => break cursor + 1,
                    _ => cursor = advance(text, cursor),
                }
            }
        }
        b'\x60' => {
            let mut cursor = start + 1;
            loop {
                let byte = *bytes
                    .get(cursor)
                    .context("a template literal is not terminated")?;
                match byte {
                    b'\\' => {
                        let escaped = cursor
                            .checked_add(1)
                            .filter(|escaped| *escaped < text.len())
                            .context("a template escape is truncated")?;
                        cursor = advance(text, escaped);
                    }
                    b'$' if bytes.get(cursor + 1) == Some(&b'{') => {
                        anyhow::bail!("template expressions are outside the safe-delete subset")
                    }
                    b'\x60' => break cursor + 1,
                    _ => cursor = advance(text, cursor),
                }
            }
        }
        b'/' if bytes.get(start + 1) == Some(&b'/') => bytes[start..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(bytes.len(), |offset| start + offset),
        b'/' if bytes.get(start + 1) == Some(&b'*') => bytes[start + 2..]
            .windows(2)
            .position(|window| window == b"*/")
            .map(|offset| start + offset + 4)
            .context("a block comment is not terminated")?,
        _ => return Ok(None),
    };
    Ok(Some(end))
}

pub fn skip_trivia(text: &str, mut cursor: usize) -> Result<usize> {
    loop {
        while text
            .as_bytes()
            .get(cursor)
            .is_some_and(u8::is_ascii_whitespace)
        {
            cursor += 1;
        }
        match opaque_end(text, cursor)? {
            Some(end) if text.as_bytes().get(cursor) == Some(&b'/') => cursor = end,
            _ => return Ok(cursor),
        }
    }
}

pub fn keyword_at(text: &str, start: usize, word: &str) -> bool {
    text.get(start..start + word.len()) == Some(word)
        && token_boundary(text.as_bytes(), start, word.len())
}

pub fn ascii_identifier_end(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let first = *bytes.get(start)?;
    if !(first.is_ascii_alphabetic() || matches!(first, b'_' | b'$')) {
        return None;
    }
    let mut end = start + 1;
    while bytes
        .get(end)
        .is_some_and(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'_' | b'$'))
    {
        end += 1;
    }
    Some(end)
}

pub fn ascii_identifier(name: &str) -> bool {
    ascii_identifier_end(name, 0) == Some(name.len())
        && !matches!(
            name,
            "await"
                | "break"
                | "case"
                | "catch"
                | "class"
                | "const"
                | "continue"
                | "debugger"
                | "default"
                | "delete"
                | "do"
                | "else"
                | "enum"
                | "export"
                | "extends"
                | "false"
                | "finally"
                | "for"
                | "function"
                | "if"
                | "import"
                | "in"
                | "instanceof"
                | "let"
                | "new"
                | "null"
                | "return"
                | "super"
                | "switch"
                | "this"
                | "throw"
                | "true"
                | "try"
                | "typeof"
                | "var"
                | "void"
                | "while"
                | "with"
                | "yield"
        )
}

pub fn token_boundary(bytes: &[u8], start: usize, len: usize) -> bool {
    let identifier =
        |byte: u8| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$') || byte >= 0x80;
    !start
        .checked_sub(1)
        .and_then(|index| bytes.get(index))
        .is_some_and(|byte| identifier(*byte))
        && !bytes.get(start + len).is_some_and(|byte| identifier(*byte))
}

pub fn pair(open: u8, close: u8) -> bool {
    matches!((open, close), (b'(', b')') | (b'[', b']') | (b'{', b'}'))
}

pub fn advance(text: &str, cursor: usize) -> usize {
    cursor
        + text[cursor..]
            .chars()
            .next()
            .map(char::len_utf8)
            .unwrap_or(1)
}
