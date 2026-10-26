/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{
    Syntax, expressions, regions,
    tokens::{pairs, tokens},
};
use anyhow::{Context, Result};
use std::ops::Range;

/// Find the actual body, not the parameter-list tail. All offsets are checked before slicing;
/// document symbols have UTF-16 positions and may put the entire declaration on one line.
pub(super) fn occurrences(
    text: &str,
    syntax: Syntax,
    close: usize,
    declaration_end: usize,
    selected: Range<usize>,
    all: bool,
    name: &str,
    casts: &[super::c_templates::Cast],
) -> Result<Vec<Range<usize>>> {
    anyhow::ensure!(
        close < declaration_end && declaration_end <= text.len(),
        "invalid function body boundary"
    );
    let mut ts = tokens(&text[close + 1..declaration_end], syntax)?;
    for t in &mut ts {
        t.start += close + 1;
        t.end += close + 1;
    }
    let paired = pairs(&ts)?;
    let body = if syntax == Syntax::Python {
        let colon = ts
            .iter()
            .enumerate()
            .find(|(_, t)| t.start > close && t.text == ":")
            .context("function body has no colon")?
            .0;
        colon + 1..ts.len()
    } else {
        let end = ts.len().checked_sub(1).context("function has no body")?;
        anyhow::ensure!(
            ts[end].text == "}",
            "function body does not end at its declaration boundary"
        );
        let open = paired[end].context("function body has no opening brace")?;
        anyhow::ensure!(
            ts[open].start > close,
            "function body precedes its parameter list"
        );
        open + 1..end
    };
    let results = if syntax == Syntax::Go {
        &ts[..body.start - 1]
    } else {
        &[]
    };
    let body = &ts[body];
    let paired = pairs(body)?;
    let (mut allowed, mut bindings) = regions::executable(text, body, &paired, syntax);
    if syntax == Syntax::Go {
        super::go_bindings::classify(
            text,
            close,
            results,
            body,
            &paired,
            &mut allowed,
            &mut bindings,
        )?;
    }
    let (nodes, uncertain) = expressions::nodes(body, &allowed, syntax, casts);
    let selection = nodes
        .iter()
        .find(|n| body[n.start].start == selected.start && body[n.end - 1].end == selected.end)
        .context("the selection is not a complete executable expression in this function body; unsupported grammar is refused")?;
    anyhow::ensure!(
        !all || uncertain.is_empty(),
        "incomplete expression coverage: unsupported executable statement or template interpolation; replace_all refused before writes"
    );
    let identity = |name: &str, at: usize| {
        bindings
            .iter()
            .enumerate()
            .filter(|(_, b)| b.name == name && b.from < at && at < b.end)
            .max_by_key(|(_, b)| b.from)
            .map(|(id, _)| id)
    };
    anyhow::ensure!(
        identity(name, selection.start).is_none(),
        "the new parameter is shadowed at the selected expression"
    );
    let equivalent = |candidate: &Range<usize>| {
        let a = &body[selection.clone()];
        let b = &body[candidate.clone()];
        a.len() == b.len()
            && a.iter().zip(b).enumerate().all(|(n, (x, y))| {
                x.text == y.text
                    && (x.literal
                        // C/C++ equality is proved with analyzer definitions before edits.
                        || syntax.is_c_family()
                        || identity(x.text, selection.start + n)
                            == identity(y.text, candidate.start + n))
            })
    };
    let mut hits = Vec::new();
    for node in nodes
        .iter()
        .filter(|n| if all { equivalent(n) } else { *n == selection })
    {
        if identity(name, node.start).is_some() {
            continue;
        }
        let range = body[node.start].start..body[node.end - 1].end;
        // Replacing a node must not swallow comments between its tokens.
        let has_comments = body[node.clone()].windows(2).any(|w| {
            let gap = &text[w[0].end..w[1].start];
            if syntax == Syntax::Python {
                !gap.replace("\\\r\n", "")
                    .replace("\\\n", "")
                    .trim()
                    .is_empty()
            } else {
                !gap.trim().is_empty()
            }
        });
        if !has_comments {
            hits.push(range);
        }
    }
    anyhow::ensure!(
        hits.contains(&selected),
        "the selected expression contains comment trivia that must be preserved"
    );
    hits.sort_by_key(|r| r.start);
    hits.dedup();
    Ok(hits)
}
