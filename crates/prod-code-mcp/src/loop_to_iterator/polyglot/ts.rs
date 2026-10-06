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

/// Recognises a TypeScript / JavaScript loop replacement.
pub fn recognise_ts(text: &str, at: usize) -> Result<PolyglotLoop> {
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
    let before_for = text[for_line_start..for_at].trim();
    anyhow::ensure!(!before_for.ends_with("await"), "the loop's header awaits");

    let open_paren = at_depth_zero(text, for_at + 4, "(").context("the `for` has no `(`")?;
    let close_paren = crate::parameter_object::matching_bracket(text, open_paren)
        .context("the `for` header `(...)` is not closed")?;
    let header = text[open_paren + 1..close_paren].trim();
    anyhow::ensure!(
        header.contains(" of "),
        "only `for...of` loops over collections are supported"
    );
    let (lhs, source) = header
        .split_once(" of ")
        .context("expected `of` in for-of loop")?;
    let source = source.trim().to_string();
    let pattern = lhs
        .strip_prefix("const ")
        .or_else(|| lhs.strip_prefix("let "))
        .or_else(|| lhs.strip_prefix("var "))
        .unwrap_or(lhs)
        .trim()
        .to_string();

    let open_brace =
        at_depth_zero(text, close_paren + 1, "{").context("the loop has no body `{`")?;
    let close_brace = crate::parameter_object::matching_bracket(text, open_brace)
        .context("the loop's body is not closed")?;
    let body = text[open_brace + 1..close_brace].trim();

    let before_loop = text[..for_line_start].trim_end_matches(['\n', ' ', '\t']);
    let dec_start = before_loop.rfind('\n').map_or(0, |i| i + 1);
    let dec_line = before_loop[dec_start..].trim();
    let dec_trimmed = dec_line.trim_end_matches(';').trim();
    let after_kw = dec_trimmed
        .strip_prefix("let ")
        .or_else(|| dec_trimmed.strip_prefix("const "))
        .or_else(|| dec_trimmed.strip_prefix("var "))
        .context(
            "the statement above the loop is not a variable declaration (`let`/`const`/`var`)",
        )?;
    let (lhs_dec, init) = after_kw
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

    for word in ["continue", "return", "throw", "yield"] {
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
                let rest_trim = rest.trim_start();
                let paren_open = rest_trim.find('(').context("the `if` has no `(`")?;
                let paren_close = crate::parameter_object::matching_bracket(rest_trim, paren_open)
                    .context("the `if` condition is not closed")?;
                let cond_str = rest_trim[paren_open + 1..paren_close].trim().to_string();
                let after_paren = rest_trim[paren_close + 1..].trim_start();
                let brace = after_paren.find('{').context("the `if` has no body")?;
                let inner_open = body.len() - after_paren.len() + brace;
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
        let assign = parts[0];
        let (assign_lhs, assign_rhs) = assign
            .split_once('=')
            .context("expected assignment before break")?;
        anyhow::ensure!(
            assign_lhs.trim() == acc,
            "assignment target is not the accumulator"
        );
        let rhs = assign_rhs.trim();
        if init == "null" || init == "undefined" {
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
                let rest_trim = rest.trim_start();
                let paren_open = rest_trim.find('(').context("the `if` has no `(`")?;
                let paren_close = crate::parameter_object::matching_bracket(rest_trim, paren_open)
                    .context("the `if` condition is not closed")?;
                let cond_str = rest_trim[paren_open + 1..paren_close].trim().to_string();
                let after_paren = rest_trim[paren_close + 1..].trim_start();
                let brace = after_paren.find('{').context("the `if` has no body")?;
                let inner_open = body.len() - after_paren.len() + brace;
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
            } else if let Some(args) = rest.strip_prefix(".push(") {
                let v = args.strip_suffix(')').context("malformed push call")?;
                anyhow::ensure!(
                    init == "[]" || init == "new Array()" || init == "Array()",
                    "`{acc}` does not start empty (`{init}`)"
                );
                Shape::Collect {
                    cond,
                    value: v.trim().to_string(),
                }
            } else {
                anyhow::bail!("the loop's body is not `{acc} += …;` or `{acc}.push(…);`");
            }
        } else {
            anyhow::bail!("the loop's body is not `{acc} += …;` or `{acc}.push(…);`");
        }
    };

    let statement = match shape {
        Shape::Sum { cond: None, value } if value == pattern => {
            format!("const {acc} = {source}.reduce((acc, {pattern}) => acc + {pattern}, 0);")
        }
        Shape::Sum { cond: None, value } => {
            format!("const {acc} = {source}.reduce((acc, {pattern}) => acc + ({value}), 0);")
        }
        Shape::Sum {
            cond: Some(c),
            value,
        } => {
            format!(
                "const {acc} = {source}.filter({pattern} => {c}).reduce((acc, {pattern}) => acc + ({value}), 0);"
            )
        }
        Shape::Count { cond } => {
            format!("const {acc} = {source}.filter({pattern} => {cond}).length;")
        }
        Shape::Collect { cond: None, value } if value == pattern => {
            format!("const {acc} = {source}.map({pattern} => {pattern});")
        }
        Shape::Collect { cond: None, value } => {
            format!("const {acc} = {source}.map({pattern} => {value});")
        }
        Shape::Collect {
            cond: Some(c),
            value,
        } => {
            format!("const {acc} = {source}.filter({pattern} => {c}).map({pattern} => {value});")
        }
        Shape::Find { cond, value } if value == pattern => {
            format!("const {acc} = {source}.find({pattern} => {cond}) ?? null;")
        }
        Shape::Find { cond, value } => {
            format!(
                "const {acc} = (() => {{ let matched = false; const found = {source}.find({pattern} => {{ const yes = {cond}; if (yes) matched = true; return yes; }}); return matched ? [found].map({pattern} => {value})[0] ?? null : null; }})();"
            )
        }
        Shape::Any { cond } => {
            format!("const {acc} = {source}.some({pattern} => {cond});")
        }
        Shape::All { cond } => {
            format!("const {acc} = {source}.every({pattern} => {cond});")
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
