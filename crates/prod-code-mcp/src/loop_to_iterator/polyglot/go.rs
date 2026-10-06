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

/// Recognises a Go loop replacement.
pub fn recognise_go(text: &str, at: usize) -> Result<PolyglotLoop> {
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
    let (lhs, source) = header
        .split_once(" range ")
        .context("expected `range` in for loop")?;
    let source = source.trim().to_string();
    let range_vars: Vec<&str> = lhs.split(',').map(str::trim).collect();
    anyhow::ensure!(
        (1..=2).contains(&range_vars.len()) && !range_vars[0].is_empty(),
        "Go range header must name one or two variables"
    );
    let pattern = if range_vars.len() == 2 {
        range_vars[1].to_string()
    } else {
        range_vars[0].to_string()
    };
    let pattern = pattern
        .strip_suffix(":=")
        .unwrap_or(&pattern)
        .trim()
        .to_string();

    let close_brace = crate::parameter_object::matching_bracket(text, open_brace)
        .context("the loop's body is not closed")?;
    let body = text[open_brace + 1..close_brace].trim();
    if range_vars.len() == 2 && range_vars[0] != "_" {
        anyhow::ensure!(
            whole_word_count(body, range_vars[0]) == 0,
            "the loop body uses range index/key `{}`, which the iterator conversion cannot preserve",
            range_vars[0]
        );
    }
    let range_binding = if range_vars.len() == 2 {
        format!("_, {pattern}")
    } else {
        pattern.clone()
    };

    let before_loop = text[..for_line_start].trim_end_matches(['\n', ' ', '\t']);
    let dec_start = before_loop.rfind('\n').map_or(0, |i| i + 1);
    let dec_line = before_loop[dec_start..].trim();
    let dec_trimmed = dec_line.trim_end_matches(';').trim();
    let (acc, init) = if let Some((a, i)) = dec_trimmed.split_once(":=") {
        (a.trim().to_string(), i.trim())
    } else if let Some(rest) = dec_trimmed.strip_prefix("var ") {
        if let Some((lhs, i)) = rest.split_once('=') {
            let a = lhs.split_whitespace().next().unwrap_or(lhs).trim();
            (a.to_string(), i.trim())
        } else {
            let parts: Vec<&str> = rest.split_whitespace().collect();
            anyhow::ensure!(!parts.is_empty(), "malformed var declaration");
            (parts[0].to_string(), "0")
        }
    } else {
        anyhow::bail!("the statement above the loop is not a variable declaration");
    };
    anyhow::ensure!(
        !acc.is_empty() && acc.chars().all(is_ident),
        "`{acc}` is not a single variable"
    );

    for word in ["continue", "return", "panic"] {
        anyhow::ensure!(
            whole_word_count(body, word) == 0,
            "the loop's body has `{word}`, which an iterator chain cannot express"
        );
    }

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
        if init == "false" && rhs == "true" {
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
            } else if rest == "++" {
                anyhow::ensure!(
                    is_zero(init),
                    "`{acc}` starts at `{init}`, not zero, so a count would lose it"
                );
                let c = cond.unwrap_or_else(|| "true".to_string());
                Shape::Count { cond: c }
            } else if let Some(args) = rest.strip_prefix("=") {
                let args = args.trim();
                if let Some(app) = args.strip_prefix("append(") {
                    let v = app.strip_suffix(')').context("malformed append call")?;
                    let (target, item) = v
                        .split_once(',')
                        .context("expected slice, item in append")?;
                    anyhow::ensure!(target.trim() == acc, "append target is not accumulator");
                    Shape::Collect {
                        cond,
                        value: item.trim().to_string(),
                    }
                } else {
                    anyhow::bail!("unsupported assignment in Go loop");
                }
            } else {
                anyhow::bail!("the loop's body is not `{acc} += …` or `{acc} = append(…)`");
            }
        } else {
            anyhow::bail!("the loop's body is not `{acc} += …` or `{acc} = append(…)`");
        }
    };

    let statement = match shape {
        Shape::Sum { cond: None, value } => {
            format!(
                "{acc} := func() int {{ s := 0; for {range_binding} := range {source} {{ s += {value} }}; return s }}()"
            )
        }
        Shape::Sum {
            cond: Some(c),
            value,
        } => {
            format!(
                "{acc} := func() int {{ s := 0; for {range_binding} := range {source} {{ if {c} {{ s += {value} }} }}; return s }}()"
            )
        }
        Shape::Count { cond } => {
            format!(
                "{acc} := func() int {{ c := 0; for {range_binding} := range {source} {{ if {cond} {{ c++ }} }}; return c }}()"
            )
        }
        Shape::Collect { cond: None, value } => {
            format!(
                "{acc} := func() []interface{{}} {{ res := make([]interface{{}}, 0); for {range_binding} := range {source} {{ res = append(res, {value}) }}; return res }}()"
            )
        }
        Shape::Collect {
            cond: Some(c),
            value,
        } => {
            format!(
                "{acc} := func() []interface{{}} {{ res := make([]interface{{}}, 0); for {range_binding} := range {source} {{ if {c} {{ res = append(res, {value}) }} }}; return res }}()"
            )
        }
        Shape::Any { cond } => {
            format!(
                "{acc} := func() bool {{ for {range_binding} := range {source} {{ if {cond} {{ return true }} }}; return false }}()"
            )
        }
        Shape::All { cond } => {
            format!(
                "{acc} := func() bool {{ for {range_binding} := range {source} {{ if !({cond}) {{ return false }} }}; return true }}()"
            )
        }
        _ => anyhow::bail!("unsupported shape for Go"),
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
