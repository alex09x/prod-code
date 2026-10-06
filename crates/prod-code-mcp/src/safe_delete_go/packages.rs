/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::lex::{ascii_identifier_end, matching, opaque_end, skip_trivia, token_boundary};
use super::types::same_file;

pub(crate) fn package_sources(
    file: &Path,
    declaration_text: &str,
) -> Result<BTreeMap<PathBuf, String>> {
    let directory = file
        .parent()
        .with_context(|| format!("{} has no containing package directory", file.display()))?;
    let package = go_package_name(declaration_text)
        .with_context(|| format!("cannot identify the Go package in {}", file.display()))?;
    let mut sources = BTreeMap::new();
    for entry in std::fs::read_dir(directory).with_context(|| {
        format!(
            "cannot inspect Go package directory {}",
            directory.display()
        )
    })? {
        let entry = entry.with_context(|| {
            format!(
                "cannot inspect an entry in Go package directory {}",
                directory.display()
            )
        })?;
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "go") {
            continue;
        }
        let kind = entry
            .file_type()
            .with_context(|| format!("cannot inspect {}", path.display()))?;
        anyhow::ensure!(
            !kind.is_symlink(),
            "linked Go source {} cannot be inspected",
            path.display()
        );
        if !kind.is_file() {
            continue;
        }
        let source = if same_file(file, &path) {
            declaration_text.to_string()
        } else {
            std::fs::read_to_string(&path)
                .with_context(|| format!("cannot read {}", path.display()))?
        };
        if go_package_name(&source) == Some(package) {
            sources.insert(path, source);
        }
    }
    anyhow::ensure!(
        sources.keys().any(|path| same_file(path, file)),
        "the declaring Go source disappeared from its package"
    );
    Ok(sources)
}

pub(crate) fn go_package_name(text: &str) -> Option<&str> {
    let at = skip_trivia(text, 0).ok()?;
    if !text[at..].starts_with("package") || !token_boundary(text.as_bytes(), at, 7) {
        return None;
    }
    let start = skip_trivia(text, at + 7).ok()?;
    let end = ascii_identifier_end(text, start)?;
    Some(&text[start..end])
}

/// Returns `(alias, generic)` for every package-level declaration of `wanted`.
pub(crate) fn type_declarations(text: &str, wanted: &str) -> Vec<(bool, bool)> {
    let bytes = text.as_bytes();
    let (mut braces, mut brackets, mut parens, mut cursor) = (0usize, 0usize, 0usize, 0usize);
    let mut found = Vec::new();
    while cursor < bytes.len() {
        if let Ok(Some(end)) = opaque_end(text, cursor) {
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
                let Ok(start) = skip_trivia(text, cursor + 4) else {
                    return found;
                };
                if bytes.get(start) == Some(&b'(') {
                    let Ok(close) = matching(text, start) else {
                        return found;
                    };
                    grouped_type_declarations(text, start, close, wanted, &mut found);
                    cursor = close + 1;
                    continue;
                }
                if let Some(end) = ascii_identifier_end(text, start) {
                    if &text[start..end] == wanted {
                        found.push(declaration_shape(text, end));
                    }
                    cursor = end;
                    continue;
                }
            }
            _ => {}
        }
        cursor += 1;
    }
    found
}

fn grouped_type_declarations(
    text: &str,
    open: usize,
    close: usize,
    wanted: &str,
    found: &mut Vec<(bool, bool)>,
) {
    let bytes = text.as_bytes();
    let (mut braces, mut brackets, mut parens) = (0usize, 0usize, 0usize);
    let (mut cursor, mut spec_start) = (open + 1, true);
    while cursor < close {
        if let Ok(Some(end)) = opaque_end(text, cursor) {
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
            && let Some(end) = ascii_identifier_end(text, cursor)
        {
            if &text[cursor..end] == wanted {
                found.push(declaration_shape(text, end));
            }
            spec_start = false;
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
            b';' if braces == 0 && brackets == 0 && parens == 0 => spec_start = true,
            b'\n' if braces == 0 && brackets == 0 && parens == 0 => spec_start = true,
            _ => {}
        }
        cursor += 1;
    }
}

fn declaration_shape(text: &str, name_end: usize) -> (bool, bool) {
    let Ok(after) = skip_trivia(text, name_end) else {
        return (false, true);
    };
    if text.as_bytes().get(after) == Some(&b'=') {
        return (true, false);
    }
    if text.as_bytes().get(after) != Some(&b'[') {
        return (false, false);
    }
    let Ok(close) = matching(text, after) else {
        return (false, true);
    };
    let Ok(tail) = skip_trivia(text, close + 1) else {
        return (false, true);
    };
    (
        text.as_bytes().get(tail) == Some(&b'='),
        bracket_declares_type_parameters(text, after, close),
    )
}

fn bracket_declares_type_parameters(text: &str, open: usize, close: usize) -> bool {
    let Ok(start) = skip_trivia(text, open + 1) else {
        return true;
    };
    if start == close || text[start..close].starts_with("...") {
        return false;
    }
    let Ok(has_comma) = bracket_has_top_level_comma(text, start, close) else {
        return true;
    };
    if has_comma {
        return true;
    }
    let Some(name_end) = ascii_identifier_end(text, start) else {
        return false;
    };
    let Ok(after) = skip_trivia(text, name_end) else {
        return true;
    };
    if after == close {
        return false;
    }
    match text.as_bytes()[after] {
        b'.' | b'(' | b'+' | b'-' | b'*' | b'/' | b'%' | b'&' | b'|' | b'^' | b'<' | b'>' => false,
        b'~' | b'[' => true,
        byte if byte.is_ascii_alphabetic() || byte == b'_' || byte >= 0x80 => true,
        _ => true,
    }
}

fn bracket_has_top_level_comma(text: &str, mut cursor: usize, close: usize) -> Result<bool> {
    while cursor < close {
        if let Some(end) = opaque_end(text, cursor)? {
            cursor = end;
            continue;
        }
        match text.as_bytes()[cursor] {
            b'(' | b'[' | b'{' => cursor = matching(text, cursor)? + 1,
            b',' => return Ok(true),
            _ => cursor += 1,
        }
    }
    Ok(false)
}
