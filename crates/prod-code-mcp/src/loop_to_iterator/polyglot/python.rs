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

use crate::loop_to_iterator::helpers::{is_ident, is_zero, whole_word_count};
use crate::loop_to_iterator::types::{PolyglotLoop, Shape};

/// Recognises a Python loop replacement.
pub fn recognise_python(text: &str, at: usize) -> Result<PolyglotLoop> {
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
    let for_line_end = text[for_at..].find('\n').map_or(text.len(), |i| for_at + i);
    let for_line = text[for_line_start..for_line_end].trim();

    let for_indent: String = text[for_line_start..for_at]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();

    let for_header = for_line.strip_prefix("for ").context("expected `for `")?;
    let for_header = for_header
        .strip_suffix(':')
        .context("expected `:` at end of for line")?
        .trim();
    let (pattern, source) = for_header
        .split_once(" in ")
        .context("expected `in` in for loop")?;
    let pattern = pattern.trim().to_string();
    let source = source.trim().to_string();

    let before_loop = text[..for_line_start].trim_end_matches(['\n', ' ', '\t']);
    let dec_start = before_loop.rfind('\n').map_or(0, |i| i + 1);
    let dec_line = before_loop[dec_start..].trim();
    let (lhs_dec, init) = dec_line
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

    let rest = &text[for_line_end..];
    let mut body_end = for_line_end;
    let mut body_lines = Vec::new();
    let mut current_offset = for_line_end;

    for line in rest.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            current_offset += line.len();
            continue;
        }
        let line_indent: String = line
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect();
        if line_indent.len() > for_indent.len() && line.starts_with(&for_indent) {
            body_lines.push(line.trim());
            current_offset += line.len();
            body_end = current_offset;
        } else {
            break;
        }
    }

    anyhow::ensure!(!body_lines.is_empty(), "the loop has no body");
    let body = body_lines.join("\n");

    for word in ["continue", "return", "raise", "yield"] {
        anyhow::ensure!(
            whole_word_count(&body, word) == 0,
            "the loop's body has `{word}`, which an iterator chain cannot express"
        );
    }
    anyhow::ensure!(!body.contains("await "), "the loop's body can await");

    let shape = if whole_word_count(&body, "break") == 1 {
        anyhow::ensure!(
            whole_word_count(&body, &acc) == 1,
            "the loop's body uses `{acc}` for more than the one accumulating statement"
        );
        let (cond, rhs) = if let Some(rest) = body.strip_prefix("if ") {
            let (cond_part, action_part) = if let Some((c, a)) = rest.split_once(':') {
                (c.trim().to_string(), a.trim())
            } else {
                anyhow::bail!("malformed if statement in python loop");
            };
            let action_lines: Vec<&str> = action_part
                .split(['\n', ';'])
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect();
            anyhow::ensure!(
                action_lines.len() == 2 && action_lines[1] == "break",
                "expected assign and break"
            );
            let (assign_lhs, assign_rhs) = action_lines[0]
                .split_once('=')
                .context("expected assignment")?;
            anyhow::ensure!(
                assign_lhs.trim() == acc,
                "assignment target is not the accumulator"
            );
            (cond_part, assign_rhs.trim())
        } else {
            anyhow::bail!("the loop's body has `break`, which an iterator chain cannot express");
        };

        if init == "None" {
            Shape::Find {
                cond,
                value: rhs.to_string(),
            }
        } else if init == "False" && rhs == "True" {
            Shape::Any { cond }
        } else if init == "True" && rhs == "False" {
            let cond_norm = if let Some(inner) = cond.strip_prefix("not ") {
                inner.trim().to_string()
            } else {
                format!("not ({cond})")
            };
            Shape::All { cond: cond_norm }
        } else {
            anyhow::bail!("the loop's body has `break`, which an iterator chain cannot express");
        }
    } else {
        anyhow::ensure!(
            whole_word_count(&body, "break") == 0,
            "the loop's body has `break`, which an iterator chain cannot express"
        );
        anyhow::ensure!(
            whole_word_count(&body, &acc) == 1,
            "the loop's body uses `{acc}` for more than the one accumulating statement"
        );
        let (cond, stmt) = if let Some(rest) = body.strip_prefix("if ") {
            let (c, a) = rest
                .split_once(':')
                .context("expected `:` in if statement")?;
            (Some(c.trim().to_string()), a.trim())
        } else {
            (None, body.as_str())
        };

        let stmt_trimmed = stmt.trim();
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
                    init == "[]" || init == "list()",
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
            format!("{acc} = sum({source})")
        }
        Shape::Sum { cond: None, value } => {
            format!("{acc} = sum({value} for {pattern} in {source})")
        }
        Shape::Sum {
            cond: Some(c),
            value,
        } => {
            format!("{acc} = sum({value} for {pattern} in {source} if {c})")
        }
        Shape::Count { cond } => {
            format!("{acc} = sum(1 for {pattern} in {source} if {cond})")
        }
        Shape::Collect { cond: None, value } => {
            format!("{acc} = [{value} for {pattern} in {source}]")
        }
        Shape::Collect {
            cond: Some(c),
            value,
        } => {
            format!("{acc} = [{value} for {pattern} in {source} if {c}]")
        }
        Shape::Find { cond, value } => {
            format!("{acc} = next(({value} for {pattern} in {source} if {cond}), None)")
        }
        Shape::Any { cond } => {
            format!("{acc} = any({cond} for {pattern} in {source})")
        }
        Shape::All { cond } => {
            format!("{acc} = all({cond} for {pattern} in {source})")
        }
    };

    let indent: String = text[dec_start..]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    let replacement = format!("{indent}{statement}");

    Ok(PolyglotLoop {
        start: dec_start,
        end: body_end,
        indent,
        replacement,
        statement,
    })
}
