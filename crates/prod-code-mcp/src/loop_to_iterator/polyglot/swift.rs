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

use crate::loop_to_iterator::helpers::{at_depth_zero, is_ident, is_zero, whole_word_count};
use crate::loop_to_iterator::types::{PolyglotLoop, Shape};

/// Recognises a Swift loop replacement.
pub fn recognise_swift(text: &str, at: usize) -> Result<PolyglotLoop> {
    let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[at..].find('\n').map_or(text.len(), |i| at + i);

    let for_at = if let Some(idx) = text[line_start..line_end].find("for ") {
        line_start + idx
    } else if let Some(idx) = text[at..].find("for ") {
        at + idx
    } else if let Some(idx) = text[..at].rfind("for ") {
        idx
    } else {
        anyhow::bail!("no `for` loop found at or near this position");
    };

    let for_line_start = text[..for_at].rfind('\n').map_or(0, |i| i + 1);
    let open_brace = at_depth_zero(text, for_at + 4, "{").context("the loop has no body `{`")?;
    let header = text[for_at + 4..open_brace].trim();
    let (pattern, source) = header
        .split_once(" in ")
        .context("expected `in` in for loop")?;
    let pattern = pattern.trim().to_string();
    let source = source.trim().to_string();

    let close_brace = crate::parameter_object::matching_bracket(text, open_brace)
        .context("the loop's body is not closed")?;
    let body = text[open_brace + 1..close_brace].trim();

    let before_loop = text[..for_line_start].trim_end_matches(['\n', ' ', '\t']);
    let dec_start = before_loop.rfind('\n').map_or(0, |i| i + 1);
    let dec_line = before_loop[dec_start..].trim();
    let dec_trimmed = dec_line.trim_end_matches(';').trim();
    let after_var = dec_trimmed
        .strip_prefix("var ")
        .context("the statement above the loop is not `var <acc> = …`")?;
    let (lhs_dec, init) = after_var
        .split_once('=')
        .context("the accumulator has no initial value")?;
    let acc = match lhs_dec.split_once(':') {
        Some((n, _)) => n.trim().to_string(),
        None => lhs_dec.trim().to_string(),
    };
    anyhow::ensure!(
        !acc.is_empty() && acc.chars().all(is_ident),
        "`{lhs_dec}` is not a single variable"
    );
    let init = init.trim();

    for word in ["continue", "return", "throw"] {
        anyhow::ensure!(
            whole_word_count(body, word) == 0,
            "the loop's body has `{word}`, which an iterator chain cannot express"
        );
    }
    anyhow::ensure!(!body.contains("await "), "the loop's body can await");

    let shape = if whole_word_count(body, "break") == 1 {
        anyhow::ensure!(
            whole_word_count(body, &acc) == 1,
            "the loop's body uses `{acc}` for more than the one accumulating statement"
        );
        let (cond, stmt) = match body.strip_prefix("if ") {
            Some(rest) => {
                let brace = rest.find('{').context("the `if` has no body")?;
                let cond_str = rest[..brace].trim().to_string();
                let inner_open = body.len() - rest.len() + brace;
                let inner_close = crate::parameter_object::matching_bracket(body, inner_open)
                    .context("the `if` is not closed")?;
                anyhow::ensure!(
                    body[inner_close + 1..].trim().is_empty(),
                    "the `if` has an `else` or is followed by more statements"
                );
                (cond_str, body[inner_open + 1..inner_close].trim())
            }
            None => {
                anyhow::bail!("the loop's body has `break`, which an iterator chain cannot express")
            }
        };
        let stmt_trimmed = stmt.trim_end_matches(';').trim();
        let parts: Vec<&str> = stmt_trimmed
            .split([';', '\n'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        anyhow::ensure!(
            parts.len() == 2 && parts[1] == "break",
            "the loop's body has `break`, which an iterator chain cannot express"
        );
        let (assign_lhs, assign_rhs) = parts[0]
            .split_once('=')
            .context("expected assignment before break")?;
        anyhow::ensure!(
            assign_lhs.trim() == acc,
            "assignment target is not the accumulator"
        );
        let rhs = assign_rhs.trim();
        if init == "nil" {
            Shape::Find {
                cond,
                value: rhs.to_string(),
            }
        } else if init == "false" && rhs == "true" {
            Shape::Any { cond }
        } else if init == "true" && rhs == "false" {
            let cond_norm = if let Some(inner) = cond.strip_prefix('!') {
                inner.trim().to_string()
            } else {
                format!("!({cond})")
            };
            Shape::All { cond: cond_norm }
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
        let (cond, stmt) = match body.strip_prefix("if ") {
            Some(rest) => {
                let brace = rest.find('{').context("the `if` has no body")?;
                let cond_str = rest[..brace].trim().to_string();
                let inner_open = body.len() - rest.len() + brace;
                let inner_close = crate::parameter_object::matching_bracket(body, inner_open)
                    .context("the `if` is not closed")?;
                anyhow::ensure!(
                    body[inner_close + 1..].trim().is_empty(),
                    "the `if` has an `else` or is followed by more statements"
                );
                (Some(cond_str), body[inner_open + 1..inner_close].trim())
            }
            None => (None, body),
        };
        let stmt_trimmed = stmt.trim_end_matches(';').trim();
        if let Some(rest) = stmt_trimmed.strip_prefix(&acc) {
            let rest = rest.trim_start();
            if let Some(val) = rest.strip_prefix("+=") {
                let v = val.trim();
                anyhow::ensure!(
                    is_zero(init),
                    "`{acc}` starts at `{init}`, not zero, so a sum would lose it"
                );
                if let Some(c) = cond {
                    if v == "1" {
                        Shape::Count { cond: c }
                    } else {
                        Shape::Sum {
                            cond: Some(c),
                            value: v.to_string(),
                        }
                    }
                } else {
                    Shape::Sum {
                        cond: None,
                        value: v.to_string(),
                    }
                }
            } else if let Some(args) = rest.strip_prefix(".append(") {
                let v = args.strip_suffix(')').context("malformed append call")?;
                anyhow::ensure!(
                    init == "[]" || init.ends_with("()") || init.ends_with("[]"),
                    "`{acc}` does not start empty (`{init}`)"
                );
                Shape::Collect {
                    cond,
                    value: v.trim().to_string(),
                }
            } else {
                anyhow::bail!("the loop's body is not `{acc} += …` or `{acc}.append(…)`");
            }
        } else {
            anyhow::bail!("the loop's body is not `{acc} += …` or `{acc}.append(…)`");
        }
    };

    let statement = match shape {
        Shape::Sum { cond: None, value } if value == pattern => {
            format!("let {acc} = {source}.reduce(0, +)")
        }
        Shape::Sum { cond: None, value } => {
            format!("let {acc} = {source}.reduce(0) {{ _acc, {pattern} in _acc + ({value}) }}")
        }
        Shape::Sum {
            cond: Some(c),
            value,
        } => {
            format!(
                "let {acc} = {source}.filter {{ {pattern} in {c} }}.reduce(0) {{ _acc, {pattern} in _acc + ({value}) }}"
            )
        }
        Shape::Count { cond } => {
            format!("let {acc} = {source}.filter {{ {pattern} in {cond} }}.count")
        }
        Shape::Collect { cond: None, value } if value == pattern => {
            format!("let {acc} = {source}.map {{ {pattern} }}")
        }
        Shape::Collect { cond: None, value } => {
            format!("let {acc} = {source}.map {{ {pattern} in {value} }}")
        }
        Shape::Collect {
            cond: Some(c),
            value,
        } => {
            format!(
                "let {acc} = {source}.filter {{ {pattern} in {c} }}.map {{ {pattern} in {value} }}"
            )
        }
        Shape::Find { cond, value } if value == pattern => {
            format!("let {acc} = {source}.first(where: {{ {pattern} in {cond} }})")
        }
        Shape::Find { cond, value } => {
            format!(
                "let {acc} = {source}.first(where: {{ {pattern} in {cond} }}).map {{ {pattern} in {value} }}"
            )
        }
        Shape::Any { cond } => {
            format!("let {acc} = {source}.contains(where: {{ {pattern} in {cond} }})")
        }
        Shape::All { cond } => {
            format!("let {acc} = {source}.allSatisfy {{ {pattern} in {cond} }}")
        }
    };

    let indent: String = text[dec_start..]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    let replacement = format!("{indent}{statement}");

    Ok(PolyglotLoop {
        start: dec_start,
        end: close_brace + 1,
        indent,
        replacement,
        statement,
    })
}
