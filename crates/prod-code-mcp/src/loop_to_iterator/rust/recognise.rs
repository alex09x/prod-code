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

use crate::loop_to_iterator::helpers::{
    accumulation, at_depth_zero, is_ident, is_zero, whole_word_count,
};
use crate::loop_to_iterator::types::{AccumulatorLoop, Shape};

/// Recognises the loop whose `for` is on the line holding `at`, with the `let mut` just above.
pub fn recognise(text: &str, at: usize) -> Result<AccumulatorLoop> {
    let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[at..].find('\n').map_or(text.len(), |i| at + i);
    let for_at = text[line_start..line_end]
        .match_indices("for ")
        .map(|(i, _)| line_start + i)
        .find(|i| !text[..*i].chars().next_back().is_some_and(is_ident))
        .context("no `for` loop on this line")?;
    let in_at = at_depth_zero(text, for_at + 4, " in ").context("the `for` has no `in`")?;
    let pattern = text[for_at + 4..in_at].trim().to_string();
    let open = at_depth_zero(text, in_at + 4, "{").context("the loop has no body")?;
    let source = text[in_at + 4..open].trim().to_string();
    let close = crate::parameter_object::matching_bracket(text, open)
        .context("the loop's body is not closed")?;
    let body = text[open + 1..close].trim();

    // The statement just above: `let mut acc = init;` or `let mut acc: T = init;`, one line.
    let before = text[..line_start].trim_end_matches(['\n', ' ', '\t']);
    let let_start = before.rfind('\n').map_or(0, |i| i + 1);
    let let_line = before[let_start..].trim();
    let rest = let_line
        .strip_prefix("let mut ")
        .context("the statement above the loop is not `let mut <accumulator> = …;`")?;
    let rest = rest
        .strip_suffix(';')
        .context("the accumulator's declaration is not one line")?;
    let (lhs, init) = rest
        .split_once(" = ")
        .context("the accumulator has no start value")?;
    let (acc, declared) = match lhs.split_once(':') {
        Some((n, t)) => (n.trim().to_string(), Some(t.trim().to_string())),
        None => (lhs.trim().to_string(), None),
    };
    anyhow::ensure!(
        !acc.is_empty() && acc.chars().all(is_ident),
        "`{lhs}` is not a single variable"
    );
    let init = init.trim();

    for word in ["continue", "return"] {
        anyhow::ensure!(
            whole_word_count(body, word) == 0,
            "the loop's body has `{word}`, which an iterator chain cannot express"
        );
    }
    anyhow::ensure!(
        !body.contains('?') && !body.contains(".await"),
        "the loop's body can leave early (`?`) or await"
    );

    let shape = if whole_word_count(body, "break") == 1 {
        let (cond, stmt) = match body.strip_prefix("if ") {
            Some(rest) => {
                let brace = at_depth_zero(rest, 0, "{").context("the `if` has no body")?;
                let inner_open = body.len() - rest.len() + brace;
                let inner_close = crate::parameter_object::matching_bracket(body, inner_open)
                    .context("the `if` is not closed")?;
                anyhow::ensure!(
                    body[inner_close + 1..].trim().is_empty(),
                    "the loop's body has `break`, which an iterator chain cannot express"
                );
                (
                    rest[..brace].trim().to_string(),
                    body[inner_open + 1..inner_close].trim(),
                )
            }
            None => {
                anyhow::bail!("the loop's body has `break`, which an iterator chain cannot express")
            }
        };
        let stmt_trimmed = stmt.trim_end_matches(';').trim();
        let parts: Vec<&str> = stmt_trimmed
            .split(';')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        anyhow::ensure!(
            parts.len() == 2 && parts[1] == "break",
            "the loop's body has `break`, which an iterator chain cannot express"
        );
        anyhow::ensure!(
            whole_word_count(body, &acc) == 1,
            "the loop's body uses `{acc}` for more than the one accumulating statement"
        );
        let assign = parts[0];
        let (assign_lhs, assign_rhs) = assign
            .split_once('=')
            .context("expected assignment before break")?;
        anyhow::ensure!(
            assign_lhs.trim() == acc,
            "assignment target is not the accumulator"
        );
        let rhs = assign_rhs.trim();
        if init == "None" && rhs.starts_with("Some(") && rhs.ends_with(')') {
            let val = rhs[5..rhs.len() - 1].trim();
            Shape::Find {
                cond,
                value: val.to_string(),
            }
        } else if init == "false" && rhs == "true" {
            Shape::Any { cond }
        } else if init == "true" && rhs == "false" {
            Shape::All {
                cond: format!("!({cond})"),
            }
        } else {
            anyhow::bail!("the loop's body has `break`, which an iterator chain cannot express");
        }
    } else {
        anyhow::ensure!(
            whole_word_count(body, "break") == 0,
            "the loop's body has `break`, which an iterator chain cannot express"
        );
        anyhow::ensure!(
            whole_word_count(body, &acc) == 1,
            "the loop's body uses `{acc}` for more than the one accumulating statement"
        );

        // One statement, or one `if` holding one statement.
        let (cond, stmt) = match body.strip_prefix("if ") {
            Some(rest) => {
                let brace = at_depth_zero(rest, 0, "{").context("the `if` has no body")?;
                let inner_open = body.len() - rest.len() + brace;
                let inner_close = crate::parameter_object::matching_bracket(body, inner_open)
                    .context("the `if` is not closed")?;
                anyhow::ensure!(
                    body[inner_close + 1..].trim().is_empty(),
                    "the `if` has an `else` or is followed by more statements"
                );
                (
                    Some(rest[..brace].trim().to_string()),
                    body[inner_open + 1..inner_close].trim(),
                )
            }
            None => (None, body),
        };
        anyhow::ensure!(
            stmt.matches(';').count() <= 1,
            "the loop's body has more than one statement"
        );
        let (pushes, value) = accumulation(stmt, &acc)
            .with_context(|| format!("the loop's body is not `{acc} += …;` or `{acc}.push(…);`"))?;
        if pushes {
            anyhow::ensure!(
                matches!(init, "Vec::new()" | "vec![]") || init.starts_with("Vec::with_capacity("),
                "`{acc}` does not start empty (`{init}`), so a collected vector would lose it"
            );
            Shape::Collect { cond, value }
        } else {
            anyhow::ensure!(
                is_zero(init),
                "`{acc}` starts at `{init}`, not zero, so a sum would lose it"
            );
            match cond {
                Some(cond) if value == "1" => Shape::Count { cond },
                cond => Shape::Sum { cond, value },
            }
        }
    };
    let indent: String = text[let_start..]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    Ok(AccumulatorLoop {
        start: let_start,
        end: close + 1,
        indent,
        acc,
        declared,
        pattern,
        source,
        shape,
    })
}
