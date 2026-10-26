/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::semantic::Source;
use crate::parameter_object::{
    Language,
    syntax::{close_in, walk_code},
};
use anyhow::{Context, Result, ensure};
use std::collections::BTreeMap;

/// Only analyzer-confirmed keys select literals. The spelling immediately before that
/// literal supplies the package alias, rather than guessing it from an import's path.
pub(crate) fn literal_edits(
    source: &mut Source<'_>,
    owner: &str,
    helper: &str,
) -> Result<Vec<(usize, usize, String)>> {
    let text = source.text;
    let mut groups: BTreeMap<usize, Vec<(usize, String)>> = BTreeMap::new();
    for (p, name) in source.positions(text) {
        if !text[p + name.len()..].trim_start().starts_with(':') {
            continue;
        }
        let mut stack = Vec::new();
        walk_code(text, 0, Language::Go, |i, c| {
            if i >= p {
                return false;
            }
            if c == b'{' {
                stack.push(i);
            }
            if c == b'}' {
                stack.pop();
            }
            true
        });
        let open = *stack
            .last()
            .context("Go field key has no enclosing literal")?;
        groups.entry(open).or_default().push((p, name));
    }
    let mut edits = Vec::new();
    for (open, keys) in groups {
        let close = close_in(text, open, Language::Go).context("unclosed Go literal")?;
        let end = text[..open].trim_end().len();
        let start = text[..end]
            .rfind(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.'))
            .map_or(0, |p| p + 1);
        let spelling = &text[start..end];
        let qualifier = spelling
            .strip_suffix(owner)
            .context("Go literal owner cannot be proved")?;
        ensure!(
            qualifier.is_empty() || qualifier.ends_with('.'),
            "ambiguous Go literal type"
        );
        if !qualifier.is_empty() {
            for name in [source.field, helper] {
                ensure!(
                    name.chars().next().is_some_and(char::is_uppercase),
                    "Go cross-package extraction requires exported requested name `{name}`"
                );
            }
        }
        let mut entries = Vec::new();
        let mut begin = open + 1;
        let mut depth = 0;
        walk_code(text, open + 1, Language::Go, |i, c| {
            if i >= close {
                return false;
            }
            match c {
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => depth -= 1,
                b',' if depth == 0 => {
                    entries.push(begin..i);
                    begin = i + 1;
                }
                _ => {}
            }
            true
        });
        entries.push(begin..close);
        let selected: Vec<_> = entries
            .iter()
            .enumerate()
            .filter(|(_, r)| keys.iter().any(|(p, _)| r.contains(p)))
            .map(|(i, r)| (i, r.clone()))
            .collect();
        ensure!(
            !selected.is_empty(),
            "Go literal keys did not match exact entries"
        );
        ensure!(
            selected.windows(2).all(|w| w[1].0 == w[0].0 + 1),
            "interleaved Go literal fields have unproved evaluation order"
        );
        let first = selected.first().unwrap().1.start;
        let last = selected.last().unwrap().1.end;
        let start = first + text[first..last].len() - text[first..last].trim_start().len();
        let end = first + text[first..last].trim_end().len();
        for (p, name) in keys {
            source.consumed(&text[p..p + name.len()]);
        }
        let body = source.rewrite(&text[start..end])?;
        edits.push((
            start,
            end,
            format!("{}: {qualifier}{helper}{{{body}}}", source.field),
        ));
    }
    Ok(edits)
}
