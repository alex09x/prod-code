/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::Result;

use super::rewrite::rewritten_args;
use super::syntax::split_args;

#[derive(Clone, PartialEq, Eq)]
pub(super) enum Replacement {
    Text(String),
    Arguments(String),
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Edit {
    pub at: usize,
    pub len: usize,
    pub replacement: Replacement,
}

impl Edit {
    pub fn text(at: usize, len: usize, text: String) -> Self {
        Self {
            at,
            len,
            replacement: Replacement::Text(text),
        }
    }

    pub fn arguments(at: usize, len: usize, spelling: String) -> Self {
        Self {
            at,
            len,
            replacement: Replacement::Arguments(spelling),
        }
    }
}

pub(super) fn rewrite(
    text: &str,
    edits: &mut Vec<Edit>,
    bundled: &[usize],
    fields: &[(String, String)],
) -> Result<String> {
    edits.sort_by_key(|e| (e.at, std::cmp::Reverse(e.len)));
    edits.dedup();
    render(text, 0, text.len(), edits, bundled, fields)
}

/// Build inner edits before rendering their containing argument list. Every offset continues
/// to address the original source; no original length is ever used on already expanded text.
/// Partial overlaps or children of opaque replacements indicate an incomplete plan and refuse.
fn render(
    text: &str,
    start: usize,
    end: usize,
    edits: &[Edit],
    bundled: &[usize],
    fields: &[(String, String)],
) -> Result<String> {
    let mut out = String::new();
    let (mut cursor, mut i) = (start, 0);
    while i < edits.len() {
        let edit = &edits[i];
        let finish = edit.at.checked_add(edit.len).filter(|e| *e <= end);
        anyhow::ensure!(
            edit.at >= cursor && finish.is_some(),
            "overlapping Rust parameter-object edits; nothing was written"
        );
        let finish = finish.unwrap();
        out.push_str(&text[cursor..edit.at]);
        let mut next = i + 1;
        while next < edits.len() && edits[next].at < finish {
            anyhow::ensure!(
                edits[next].at + edits[next].len <= finish,
                "partially overlapping Rust parameter-object edits; nothing was written"
            );
            next += 1;
        }
        match &edit.replacement {
            Replacement::Text(replacement) => {
                anyhow::ensure!(
                    next == i + 1,
                    "overlapping Rust parameter-object replacements; nothing was written"
                );
                out.push_str(replacement);
            }
            Replacement::Arguments(spelling) => {
                let inner = render(text, edit.at, finish, &edits[i + 1..next], bundled, fields)?;
                out.push_str(&rewritten_args(
                    &split_args(&inner),
                    bundled,
                    spelling,
                    fields,
                ));
            }
        }
        cursor = finish;
        i = next;
    }
    out.push_str(&text[cursor..end]);
    Ok(out)
}
