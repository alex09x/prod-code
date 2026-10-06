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

pub(crate) fn ensure_top_level(text: &str, end: usize) -> Result<()> {
    let mut stack = Vec::new();
    let mut cursor = 0usize;
    while cursor < end {
        if let Some(next) = opaque_end(text, cursor)? {
            anyhow::ensure!(
                next <= end,
                "a comment or literal overlaps the declaration range"
            );
            cursor = next;
            continue;
        }
        match text.as_bytes()[cursor] {
            open @ (b'(' | b'[' | b'{') => stack.push(open),
            close @ (b')' | b']' | b'}') => {
                let open = stack
                    .pop()
                    .context("unmatched closing delimiter before the function")?;
                anyhow::ensure!(
                    pair(open, close),
                    "mismatched delimiter before the function"
                );
            }
            _ => {}
        }
        cursor += 1;
    }
    anyhow::ensure!(
        stack.is_empty(),
        "the function is nested in another declaration or expression"
    );
    Ok(())
}

pub(crate) fn matching(text: &str, open: usize) -> Result<usize> {
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
        match text.as_bytes()[cursor] {
            next @ (b'(' | b'[' | b'{') => stack.push(next),
            close @ (b')' | b']' | b'}') => {
                let opened = stack
                    .pop()
                    .context("an unmatched delimiter closes the declaration")?;
                anyhow::ensure!(
                    pair(opened, close),
                    "a delimiter is mismatched in the declaration"
                );
                if stack.is_empty() {
                    return Ok(cursor);
                }
            }
            _ => {}
        }
        cursor += 1;
    }
    anyhow::bail!("an opening delimiter in the declaration is not closed")
}

pub(crate) fn pair(open: u8, close: u8) -> bool {
    matches!((open, close), (b'(', b')') | (b'[', b']') | (b'{', b'}'))
}

pub(crate) fn opaque_end(text: &str, start: usize) -> Result<Option<usize>> {
    let bytes = text.as_bytes();
    let Some(&first) = bytes.get(start) else {
        return Ok(None);
    };
    let end = match first {
        quote @ (b'"' | b'\'') => {
            let mut cursor = start + 1;
            loop {
                let byte = *bytes
                    .get(cursor)
                    .context("a quoted literal is not terminated")?;
                match byte {
                    b'\\' => {
                        cursor = cursor
                            .checked_add(2)
                            .context("a quoted literal escape is truncated")?;
                    }
                    b'\n' => anyhow::bail!("a quoted literal crosses a line without closing"),
                    value if value == quote => break cursor + 1,
                    _ => cursor += 1,
                }
            }
        }
        b'\x60' => bytes[start + 1..]
            .iter()
            .position(|byte| *byte == b'\x60')
            .map(|offset| start + offset + 2)
            .context("a raw string literal is not terminated")?,
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

pub(crate) fn skip_trivia(text: &str, mut cursor: usize) -> Result<usize> {
    loop {
        while text
            .as_bytes()
            .get(cursor)
            .is_some_and(u8::is_ascii_whitespace)
        {
            cursor += 1;
        }
        match opaque_end(text, cursor)? {
            Some(end) if text.as_bytes()[cursor] == b'/' => cursor = end,
            _ => return Ok(cursor),
        }
    }
}

pub(crate) fn ascii_identifier_end(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let first = *bytes.get(start)?;
    if !(first.is_ascii_alphabetic() || first == b'_') {
        return None;
    }
    let mut end = start + 1;
    while bytes
        .get(end)
        .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
    {
        end += 1;
    }
    Some(end)
}

pub(crate) fn go_identifier_end(text: &str, start: usize) -> Option<usize> {
    let mut chars = text.get(start..)?.char_indices();
    let (_, first) = chars.next()?;
    if first != '_' && !unicode_ident::is_xid_start(first) {
        return None;
    }
    let mut end = start + first.len_utf8();
    for (offset, character) in chars {
        if character != '_' && !unicode_ident::is_xid_continue(character) {
            break;
        }
        end = start + offset + character.len_utf8();
    }
    Some(end)
}

pub(crate) fn ascii_unexported_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.first().is_some_and(u8::is_ascii_lowercase)
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
}

pub(crate) fn ascii_identifier(name: &str) -> bool {
    let Some(end) = ascii_identifier_end(name, 0) else {
        return false;
    };
    end == name.len()
        && name != "_"
        && !matches!(
            name,
            "break"
                | "default"
                | "func"
                | "interface"
                | "select"
                | "case"
                | "defer"
                | "go"
                | "map"
                | "struct"
                | "chan"
                | "else"
                | "goto"
                | "package"
                | "switch"
                | "const"
                | "fallthrough"
                | "if"
                | "range"
                | "type"
                | "continue"
                | "for"
                | "import"
                | "return"
                | "var"
        )
}

pub(crate) fn token_boundary(bytes: &[u8], start: usize, len: usize) -> bool {
    let identifier = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 0x80;
    !start
        .checked_sub(1)
        .and_then(|index| bytes.get(index))
        .is_some_and(|byte| identifier(*byte))
        && !bytes.get(start + len).is_some_and(|byte| identifier(*byte))
}

pub(crate) fn trim_ascii_end(text: &str, start: usize, mut end: usize) -> usize {
    while end > start && text.as_bytes()[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    end
}
