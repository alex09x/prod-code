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

use anyhow::{Context, Result};

use crate::extract_delegate::common::is_ident;
use crate::extract_trait::lex::{lexical_code, next_code, previous_code};

/// Original byte positions from the analyzer, including constructor/pattern entries.
/// Planning all insertions against one snapshot keeps nested accesses and literals ordered.
pub(super) async fn rewrite<F: std::future::Future<Output = Result<String>>>(
    text: &str,
    references: &[(usize, String)],
    moved_ranges: &[(usize, usize)],
    fields: &[String],
    field: &str,
    helper: impl Fn(usize) -> F,
) -> Result<(String, usize)> {
    let code = lexical_code(text);
    let mut edits = BTreeMap::<usize, String>::new();
    let mut literals = BTreeMap::<usize, Vec<usize>>::new();
    let mut replaced = Vec::<(usize, usize, String)>::new();
    let mut accesses = 0;
    for (off, name) in references {
        anyhow::ensure!(
            code[*off],
            "a reference to `{name}` is inside a comment or literal; nothing was planned"
        );
        let previous = previous_code(text, &code, *off);
        if let Some(dot) = previous.filter(|i| text.as_bytes()[*i] == b'.') {
            let helper_self = if let Some((start, _)) =
                moved_ranges.iter().find(|(s, e)| *s <= *off && *off < *e)
            {
                super::method_syntax::helper_receiver(&text[*start..dot])?
            } else {
                false
            };
            if !helper_self && edits.insert(*off, format!("{field}.")).is_none() {
                accesses += 1;
            }
            continue;
        }
        let mut stack = Vec::new();
        for (i, b) in text.bytes().enumerate().take(*off) {
            if !code[i] {
                continue;
            }
            match b {
                b'{' => stack.push(i),
                b'}' => {
                    stack.pop();
                }
                _ => {}
            }
        }
        let open = stack.last().copied().context(
            "unsupported field reference outside a literal or pattern; nothing was planned",
        )?;
        literals.entry(open).or_default().push(*off);
    }
    for (open, anchors) in literals {
        let close = crate::extract_trait::lex::matching_close_brace(text, open)
            .context("unclosed literal or pattern; nothing was planned")?;
        let entries = entries(text, &code, open + 1, close)?;
        for anchor in anchors {
            anyhow::ensure!(
                entries.iter().any(|e| e.key == anchor),
                "unsupported field reference at byte {anchor}; nothing was planned"
            );
        }
        let selected: Vec<usize> = entries
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                fields.iter().any(|f| {
                    text[e.key..].starts_with(f) && !text[e.key + f.len()..].starts_with(is_ident)
                })
            })
            .map(|(i, _)| i)
            .collect();
        let first = *selected
            .first()
            .context("no moved entries in literal or pattern")?;
        let last = *selected.last().unwrap();
        anyhow::ensure!(
            selected.len() == last - first + 1,
            "interleaved moved fields would change evaluation order; rewrite the literal or pattern by hand; nothing was planned"
        );
        let helper = helper(entries[first].key).await?;
        let (from, to) = (entries[first].key, entries[last].end);
        // Entries with nothing but code and blanks between them join on one line; anything
        // else (comments, strings, nested edits) keeps its own lines and bytes.
        let plain = edits.range(from..to).next().is_none()
            && text[from..to]
                .char_indices()
                .all(|(i, c)| c.is_whitespace() || code[from + i]);
        if plain {
            let joined: Vec<&str> = entries[first..=last]
                .iter()
                .map(|e| text[e.key..e.end].trim())
                .collect();
            replaced.push((
                from,
                to,
                format!("{field}: {helper} {{ {} }}", joined.join(", ")),
            ));
        } else {
            edits
                .entry(from)
                .or_default()
                .push_str(&format!("{field}: {helper} {{ "));
            edits.entry(to).or_default().push_str(" }");
        }
    }
    let mut ops: Vec<(usize, usize, String)> =
        edits.into_iter().map(|(at, s)| (at, at, s)).collect();
    ops.extend(replaced);
    ops.sort_by_key(|(start, end, _)| (*start, *end));
    let mut result = text.to_owned();
    for (start, end, replacement) in ops.into_iter().rev() {
        result.replace_range(start..end, &replacement);
    }
    Ok((result, accesses))
}

struct Entry {
    end: usize,
    key: usize,
}

/// Only delimiters and field heads are read; values/patterns are retained byte for byte.
/// Explicit turbofish arguments balance independently of expression delimiters.
/// Ambiguous comparison/operator angles still refuse rather than swallow field commas.
fn entries(text: &str, code: &[bool], start: usize, end: usize) -> Result<Vec<Entry>> {
    let mut spans = Vec::new();
    let mut depth = 0usize;
    let mut angles = 0usize;
    let mut from = start;
    for (i, b) in text.bytes().enumerate().take(end).skip(start) {
        if !code[i] {
            continue;
        }
        match b {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth = depth.checked_sub(1).context("unbalanced literal entry")?
            }
            b'<' if depth == 0 && (angles > 0 || turbofish(text, code, i)) => angles += 1,
            b'>' if depth == 0 && angles > 0 => {
                // A function type's `->` is not the end of generic arguments.
                if !previous_code(text, code, i).is_some_and(|p| text.as_bytes()[p] == b'-') {
                    angles -= 1;
                }
            }
            b'<' | b'>' if depth == 0 => anyhow::bail!(
                "unsupported top-level angle syntax in literal or pattern; nothing was planned"
            ),
            b',' if depth == 0 && angles == 0 => {
                spans.push((from, i));
                from = i + 1;
            }
            _ => {}
        }
    }
    anyhow::ensure!(
        depth == 0 && angles == 0,
        "unbalanced literal or pattern; nothing was planned"
    );
    spans.push((from, end));
    let mut out = Vec::new();
    for (start, end) in spans {
        let Some(mut key) = next_code(text, code, start).filter(|i| *i < end) else {
            continue;
        };
        anyhow::ensure!(
            !text[key..].starts_with(".."),
            "a literal or pattern uses `..`; rewrite it by hand; nothing was planned"
        );
        for modifier in ["ref", "mut"] {
            if text[key..].starts_with(modifier)
                && !text[key + modifier.len()..].starts_with(is_ident)
            {
                key =
                    next_code(text, code, key + modifier.len()).context("missing pattern field")?;
            }
        }
        let key_end = text[key..end]
            .find(|c| !is_ident(c))
            .map_or(end, |i| key + i);
        anyhow::ensure!(
            key_end > key,
            "unsupported literal or pattern entry; nothing was planned"
        );
        if let Some(after) = next_code(text, code, key_end).filter(|i| *i < end) {
            anyhow::ensure!(
                text.as_bytes()[after] == b':' && text.as_bytes().get(after + 1) != Some(&b':'),
                "unsupported literal or pattern field; nothing was planned"
            );
        }
        out.push(Entry { end, key });
    }
    Ok(out)
}

fn turbofish(text: &str, code: &[bool], at: usize) -> bool {
    let Some(last) = previous_code(text, code, at) else {
        return false;
    };
    text.as_bytes()[last] == b':'
        && previous_code(text, code, last).is_some_and(|p| text.as_bytes()[p] == b':')
}
