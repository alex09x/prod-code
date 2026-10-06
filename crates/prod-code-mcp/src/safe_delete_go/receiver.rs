/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::lex::{go_identifier_end, matching, opaque_end, skip_trivia, token_boundary};
use super::packages::{package_sources, type_declarations};
use super::types::Receiver;

pub(crate) fn receiver_source_evidence(
    file: &Path,
    text: &str,
    receiver: &Receiver,
) -> Result<BTreeMap<PathBuf, String>> {
    let sources = package_sources(file, text)?;
    let declarations: Vec<(bool, bool)> = sources
        .values()
        .flat_map(|source| type_declarations(source, &receiver.type_name))
        .collect();
    anyhow::ensure!(
        declarations.len() == 1,
        "receiver type {} has {} package declarations instead of exactly one",
        receiver.type_name,
        declarations.len()
    );
    let (alias, generic) = declarations[0];
    anyhow::ensure!(!alias, "receiver type {} is an alias", receiver.type_name);
    anyhow::ensure!(
        !generic,
        "receiver type {} is generic or parameterized",
        receiver.type_name
    );
    let receiver_names = receiver_type_names(&sources, &receiver.type_name)?;
    for (path, source) in &sources {
        anyhow::ensure!(
            !embeds_receiver(source, &receiver_names)?,
            "receiver type {} is embedded or promoted in {}",
            receiver.type_name,
            path.display()
        );
    }
    Ok(sources)
}

pub(crate) fn receiver_type_names(
    sources: &BTreeMap<PathBuf, String>,
    receiver: &str,
) -> Result<BTreeSet<String>> {
    let aliases = sources
        .values()
        .map(|source| type_aliases(source))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let mut names = BTreeSet::from([receiver.to_string()]);
    loop {
        let mut changed = false;
        for (alias, target) in &aliases {
            if names.contains(target) {
                changed |= names.insert(alias.clone());
            }
        }
        if !changed {
            return Ok(names);
        }
    }
}

pub(crate) fn type_aliases(text: &str) -> Result<Vec<(String, String)>> {
    let bytes = text.as_bytes();
    let (mut braces, mut brackets, mut parens, mut cursor) = (0usize, 0usize, 0usize, 0usize);
    let mut aliases = Vec::new();
    while cursor < bytes.len() {
        if let Some(end) = opaque_end(text, cursor)? {
            cursor = end;
            continue;
        }
        match bytes[cursor] {
            b'{' => braces += 1,
            b'}' => braces = braces.saturating_sub(1),
            b'[' => brackets += 1,
            b']' => brackets = brackets.saturating_sub(1),
            b'(' => parens += 1,
            b')' => parens = parens.saturating_sub(1),
            b't' if braces == 0
                && brackets == 0
                && parens == 0
                && bytes[cursor..].starts_with(b"type")
                && token_boundary(bytes, cursor, 4) =>
            {
                let start = skip_trivia(text, cursor + 4)?;
                if bytes.get(start) == Some(&b'(') {
                    let close = matching(text, start)?;
                    grouped_type_aliases(text, start, close, &mut aliases)?;
                    cursor = close + 1;
                    continue;
                }
                if let Some(end) = go_identifier_end(text, start) {
                    if let Some(target) = type_alias_target(text, end)? {
                        aliases.push((text[start..end].to_string(), target));
                    }
                    cursor = end;
                    continue;
                }
                anyhow::ensure!(
                    bytes.get(start).is_none_or(|byte| *byte < 0x80),
                    "a non-ASCII type declaration cannot be inspected conservatively"
                );
            }
            _ => {}
        }
        cursor += 1;
    }
    Ok(aliases)
}

fn grouped_type_aliases(
    text: &str,
    open: usize,
    close: usize,
    aliases: &mut Vec<(String, String)>,
) -> Result<()> {
    let bytes = text.as_bytes();
    let (mut braces, mut brackets, mut parens) = (0usize, 0usize, 0usize);
    let (mut cursor, mut spec_start) = (open + 1, true);
    while cursor < close {
        if let Some(end) = opaque_end(text, cursor)? {
            if text[cursor..end].contains('\n') && braces == 0 && brackets == 0 && parens == 0 {
                spec_start = true;
            }
            cursor = end;
            continue;
        }
        if spec_start && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
            continue;
        }
        if spec_start
            && braces == 0
            && brackets == 0
            && parens == 0
            && let Some(end) = go_identifier_end(text, cursor)
        {
            if let Some(target) = type_alias_target(text, end)? {
                aliases.push((text[cursor..end].to_string(), target));
            }
            spec_start = false;
            cursor = end;
            continue;
        }
        anyhow::ensure!(
            !spec_start || bytes[cursor] < 0x80,
            "a non-ASCII grouped type declaration cannot be inspected conservatively"
        );
        match bytes[cursor] {
            b'{' => braces += 1,
            b'}' => braces = braces.saturating_sub(1),
            b'[' => brackets += 1,
            b']' => brackets = brackets.saturating_sub(1),
            b'(' => parens += 1,
            b')' => parens = parens.saturating_sub(1),
            b';' if braces == 0 && brackets == 0 && parens == 0 => spec_start = true,
            b'\n' if braces == 0 && brackets == 0 && parens == 0 => spec_start = true,
            _ => {}
        }
        cursor += 1;
    }
    Ok(())
}

fn type_alias_target(text: &str, name_end: usize) -> Result<Option<String>> {
    let mut cursor = skip_trivia(text, name_end)?;
    if text.as_bytes().get(cursor) == Some(&b'[') {
        cursor = skip_trivia(text, matching(text, cursor)? + 1)?;
    }
    if text.as_bytes().get(cursor) != Some(&b'=') {
        return Ok(None);
    }
    cursor = skip_trivia(text, cursor + 1)?;

    let mut closes = Vec::new();
    loop {
        match text.as_bytes().get(cursor) {
            Some(b'*') => cursor = skip_trivia(text, cursor + 1)?,
            Some(b'(') => {
                closes.push(matching(text, cursor)?);
                cursor = skip_trivia(text, cursor + 1)?;
            }
            _ => break,
        }
    }
    let Some(end) = go_identifier_end(text, cursor) else {
        return Ok(None);
    };
    let target = text[cursor..end].to_string();
    cursor = end;
    while let Some(close) = closes.pop() {
        cursor = skip_trivia(text, cursor)?;
        if cursor != close {
            return Ok(None);
        }
        cursor = close + 1;
    }
    Ok(Some(target))
}

pub(crate) fn embeds_receiver(text: &str, receiver_names: &BTreeSet<String>) -> Result<bool> {
    let bytes = text.as_bytes();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if let Some(end) = opaque_end(text, cursor)? {
            cursor = end;
            continue;
        }
        if bytes[cursor..].starts_with(b"struct") && token_boundary(bytes, cursor, 6) {
            let open = skip_trivia(text, cursor + 6)?;
            if bytes.get(open) == Some(&b'{') {
                let close = matching(text, open)?;
                if struct_body_embeds(&text[open + 1..close], receiver_names)? {
                    return Ok(true);
                }
                cursor = open + 1;
                continue;
            }
        }
        cursor += 1;
    }
    Ok(false)
}

fn struct_body_embeds(body: &str, receiver_names: &BTreeSet<String>) -> Result<bool> {
    let bytes = body.as_bytes();
    let (mut braces, mut brackets, mut parens, mut start, mut cursor) =
        (0usize, 0usize, 0usize, 0usize, 0usize);
    while cursor <= bytes.len() {
        if cursor == bytes.len() {
            return field_embeds(&body[start..cursor], receiver_names);
        }
        if let Some(end) = opaque_end(body, cursor)? {
            if bytes[cursor] == b'/'
                && body[cursor..end].contains('\n')
                && braces == 0
                && brackets == 0
                && parens == 0
            {
                if field_embeds(&body[start..cursor], receiver_names)? {
                    return Ok(true);
                }
                start = end;
            }
            cursor = end;
            continue;
        }
        match bytes[cursor] {
            b'{' => braces += 1,
            b'}' => braces = braces.saturating_sub(1),
            b'[' => brackets += 1,
            b']' => brackets = brackets.saturating_sub(1),
            b'(' => parens += 1,
            b')' => parens = parens.saturating_sub(1),
            b';' | b'\n' if braces == 0 && brackets == 0 && parens == 0 => {
                if field_embeds(&body[start..cursor], receiver_names)? {
                    return Ok(true);
                }
                start = cursor + 1;
            }
            _ => {}
        }
        cursor += 1;
    }
    Ok(false)
}

fn field_embeds(field: &str, receiver_names: &BTreeSet<String>) -> Result<bool> {
    let mut cursor = skip_trivia(field, 0)?;
    if field.as_bytes().get(cursor) == Some(&b'*') {
        cursor += 1;
    }
    let Some(end) = go_identifier_end(field, cursor) else {
        return Ok(false);
    };
    if !receiver_names.contains(&field[cursor..end]) {
        return Ok(false);
    }
    cursor = skip_trivia(field, end)?;
    if field.as_bytes().get(cursor) == Some(&b'[') {
        cursor = skip_trivia(field, matching(field, cursor)? + 1)?;
    }
    if cursor == field.len() {
        return Ok(true);
    }
    if matches!(field.as_bytes().get(cursor), Some(b'"' | b'\'' | b'\x60')) {
        let tag_end = opaque_end(field, cursor)?.context("an embedded field tag is malformed")?;
        return Ok(skip_trivia(field, tag_end)? == field.len());
    }
    Ok(false)
}
