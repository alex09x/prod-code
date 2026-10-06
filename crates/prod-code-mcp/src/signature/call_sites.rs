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
use std::collections::BTreeMap;
use std::path::Path;

use crate::signature::parse::offset_of;
use crate::signature::types::{CallSite, Reference};
use crate::signature::util::{display, read_caller};

/// The references that are calls, with their arguments. A reference that does not call the
/// function (a use as a value, an import) has none and is left to the reconciliation, which
/// names it; a reference whose position does not hold a name, or whose argument list does not
/// close, stops the change.
pub fn call_sites(root: &Path, refs: &[Reference]) -> Result<Vec<CallSite>> {
    let mut texts: BTreeMap<&Path, String> = BTreeMap::new();
    let mut out = Vec::new();
    for (path, l, c) in refs {
        if !texts.contains_key(path.as_path()) {
            texts.insert(path.as_path(), read_caller(path)?);
        }
        let text = &texts[path.as_path()];
        let at = format!("{}:{l}:{c}", display(root, path));
        let offset = offset_of(text, *l, *c)
            .filter(|o| text[*o..].starts_with(|ch: char| ch.is_alphanumeric() || ch == '_'))
            .with_context(|| {
                format!(
                    "the analyzer's reference {at} does not point at a name; the file may have \
                     changed since it was read. Nothing was written"
                )
            })?;
        if let Some(args) = call_arguments(text, offset).with_context(|| {
            format!("the arguments of the call at {at} could not be read; nothing was written")
        })? {
            out.push(CallSite { at, args });
        }
    }
    Ok(out)
}

/// The arguments of the call whose callee's name starts at `at` (`f(…)`, `recv.f(…)`,
/// `Type::f::<T>(…)`), split at their top-level commas. `Ok(None)` when the name is not called
/// there; an error when the argument list does not close.
pub fn call_arguments(text: &str, at: usize) -> Result<Option<Vec<String>>> {
    let Some(open) = call_open(text, at)? else {
        return Ok(None);
    };
    split_arguments(text, open)
        .map(Some)
        .context("the argument list does not close")
}

/// Where the argument list of the call whose callee's name starts at `at` opens, past a
/// turbofish. `Ok(None)` when the name is not called there; an error when the turbofish does
/// not close.
pub fn call_open(text: &str, at: usize) -> Result<Option<usize>> {
    let bytes = text.as_bytes();
    let skip_ws = |i: usize| i + (text[i..].len() - text[i..].trim_start().len());
    let mut i = at;
    while i < bytes.len()
        && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] >= 0x80)
    {
        i += 1;
    }
    i = skip_ws(i);
    if text[i..].starts_with("::") {
        let open = skip_ws(i + 2);
        if !text[open..].starts_with('<') {
            return Ok(None);
        }
        let mut depth = 0i32;
        let mut close = None;
        for (k, c) in text[open..].char_indices() {
            match c {
                '<' => depth += 1,
                '>' if !text[..open + k].ends_with('-') => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(open + k + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        i = skip_ws(close.context("the turbofish does not close")?);
    }
    Ok(text[i..].starts_with('(').then_some(i))
}

/// What starts at byte `i` of `text` and hides commas, brackets and comment markers inside it: a
/// string, raw string or character literal, or a comment. `Ok(Some((end, is_comment)))` with the
/// offset just past it, `Ok(None)` when none starts there (a lifetime has no closing quote), and
/// `Err(())` when one starts and does not close. Block comments nest, as Rust's do: the first
/// `*/` in `/* a /* b */ c */` does not end it. `from` is where the scan began.
pub fn opaque_at(text: &str, i: usize, from: usize) -> Result<Option<(usize, bool)>, ()> {
    let s = text.as_bytes();
    let ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80;
    let prev = if i > from { s[i - 1] } else { b' ' };
    match s[i] {
        // A raw string (`r"…"`, `r#"…"#`, `br"…"`): no escapes, closed by `"` and its hashes.
        b'r' if !ident(prev) || (matches!(prev, b'b' | b'c') && (i < 2 || !ident(s[i - 2]))) => {
            let mut j = i + 1;
            while s.get(j) == Some(&b'#') {
                j += 1;
            }
            if s.get(j) != Some(&b'"') {
                return Ok(None);
            }
            let close = format!("\"{}", "#".repeat(j - i - 1));
            let end = text[j + 1..].find(&close).ok_or(())? + j + 1 + close.len();
            Ok(Some((end, false)))
        }
        b'"' => {
            let mut j = i + 1;
            loop {
                match s.get(j).ok_or(())? {
                    b'\\' => j += 2,
                    b'"' => break,
                    _ => j += 1,
                }
            }
            Ok(Some((j + 1, false)))
        }
        b'\'' => {
            if s.get(i + 1) == Some(&b'\\') {
                let end = text.get(i + 3..).ok_or(())?.find('\'').ok_or(())? + i + 4;
                return Ok(Some((end, false)));
            }
            let len = text[i + 1..].chars().next().ok_or(())?.len_utf8();
            Ok((s.get(i + 1 + len) == Some(&b'\'')).then_some((i + 2 + len, false)))
        }
        b'/' if s.get(i + 1) == Some(&b'/') => Ok(Some((
            text[i..].find('\n').map_or(s.len(), |n| i + n),
            true,
        ))),
        b'/' if s.get(i + 1) == Some(&b'*') => {
            let mut depth = 0usize;
            let mut j = i;
            while j + 1 < s.len() {
                match (s[j], s[j + 1]) {
                    (b'/', b'*') => {
                        depth += 1;
                        j += 2;
                    }
                    (b'*', b'/') => {
                        depth -= 1;
                        j += 2;
                        if depth == 0 {
                            return Ok(Some((j, true)));
                        }
                    }
                    _ => j += 1,
                }
            }
            Err(())
        }
        _ => Ok(None),
    }
}

/// `expr` with each comment replaced by as many spaces as it has bytes, so that what is left is
/// code and offsets stay where they were; `None` when a literal or a comment does not close.
pub fn blank_comments(expr: &str) -> Option<String> {
    let mut out = expr.as_bytes().to_vec();
    let mut i = 0;
    while i < expr.len() {
        match opaque_at(expr, i, 0).ok()? {
            Some((end, comment)) => {
                if comment {
                    out[i..end].fill(b' ');
                }
                i = end;
            }
            None => i += 1,
        }
    }
    String::from_utf8(out).ok()
}

/// The arguments between the `(` at `open` and its `)`. Commas count only outside brackets,
/// string and character literals, comments (nested ones too) and turbofish generics
/// (`Vec::<(u8, u8)>::new()`).
pub fn split_arguments(text: &str, open: usize) -> Option<Vec<String>> {
    let s = text.as_bytes();
    let mut args = Vec::new();
    let (mut depth, mut angle) = (0i32, 0i32);
    let mut start = open + 1;
    let mut i = open + 1;
    while i < s.len() {
        if let Some((end, _)) = opaque_at(text, i, open + 1).ok()? {
            i = end;
            continue;
        }
        let c = s[i];
        let prev = if i > open + 1 { s[i - 1] } else { b' ' };
        match c {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' if depth > 0 => depth -= 1,
            b')' => {
                let last = text[start..i].trim();
                if !last.is_empty() {
                    args.push(last.to_string());
                }
                return Some(args);
            }
            b']' | b'}' => return None,
            b'<' if angle > 0 || text[..i].trim_end().ends_with("::") => angle += 1,
            b'>' if angle > 0 && prev != b'-' => angle -= 1,
            b',' if depth == 0 && angle == 0 => {
                args.push(text[start..i].trim().to_string());
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    None
}
