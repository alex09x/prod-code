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

/// Recognises a C++ loop replacement.
pub fn recognise_cpp(text: &str, at: usize) -> Result<PolyglotLoop> {
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
    let open_paren = at_depth_zero(text, for_at + 4, "(").context("the `for` has no `(`")?;
    let close_paren = crate::parameter_object::matching_bracket(text, open_paren)
        .context("the `for` header `(...)` is not closed")?;
    let header = text[open_paren + 1..close_paren].trim();
    let (decl, source) = header
        .split_once(':')
        .context("expected `:` in range-for loop")?;
    let source = source.trim().to_string();
    let pattern = decl
        .rsplit(|c: char| !is_ident(c))
        .find(|s| !s.is_empty())
        .context("could not extract loop variable")?
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
    let (lhs_dec, init) = dec_trimmed
        .split_once('=')
        .context("the accumulator has no initial value")?;
    let acc = lhs_dec
        .rsplit(|c: char| !is_ident(c))
        .find(|s| !s.is_empty())
        .context("could not extract accumulator variable")?
        .to_string();
    let accumulator_decl = lhs_dec.trim().to_string();
    let init = init.trim();
    anyhow::ensure!(
        !source.starts_with('{'),
        "a braced range initializer cannot be safely stored for a single-evaluation iterator conversion"
    );
    let mut range_name = "__prod_code_range".to_string();
    let mut suffix = 0usize;
    while text.contains(&range_name) {
        suffix += 1;
        range_name = format!("__prod_code_range_{suffix}");
    }
    let raw_acc_type = accumulator_decl
        .strip_suffix(&acc)
        .unwrap_or_default()
        .trim();
    let acc_type = raw_acc_type
        .strip_prefix("const ")
        .or_else(|| raw_acc_type.strip_prefix("volatile "))
        .unwrap_or(raw_acc_type)
        .trim();
    anyhow::ensure!(
        !acc_type.contains('&'),
        "a reference accumulator cannot be represented safely by std::accumulate"
    );
    let sum_initial = if acc_type.is_empty() || acc_type == "auto" || acc_type == "decltype(auto)" {
        init.to_string()
    } else {
        format!("static_cast<{acc_type}>({init})")
    };

    for word in ["continue", "return", "throw"] {
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
            } else {
                anyhow::bail!("the loop's body is not `{acc} += …;`");
            }
        } else {
            anyhow::bail!("the loop's body is not `{acc} += …;`");
        }
    };

    let statement = match shape {
        Shape::Sum { cond: None, value } if value == pattern => {
            format!(
                "{accumulator_decl} = std::accumulate({range_name}.begin(), {range_name}.end(), {sum_initial});"
            )
        }
        Shape::Sum { cond: None, value } => {
            format!(
                "{accumulator_decl} = std::accumulate({range_name}.begin(), {range_name}.end(), {sum_initial}, [](auto _acc, const auto& {pattern}) {{ return _acc + ({value}); }});"
            )
        }
        Shape::Sum {
            cond: Some(c),
            value,
        } => {
            format!(
                "{accumulator_decl} = std::accumulate({range_name}.begin(), {range_name}.end(), {sum_initial}, [](auto _acc, const auto& {pattern}) {{ return ({c}) ? _acc + ({value}) : _acc; }});"
            )
        }
        Shape::Count { cond } => {
            format!(
                "const auto {acc} = std::count_if({range_name}.begin(), {range_name}.end(), [](const auto& {pattern}) {{ return {cond}; }});"
            )
        }
        Shape::Any { cond } => {
            format!(
                "const bool {acc} = std::any_of({range_name}.begin(), {range_name}.end(), [](const auto& {pattern}) {{ return {cond}; }});"
            )
        }
        Shape::All { cond } => {
            format!(
                "const bool {acc} = std::all_of({range_name}.begin(), {range_name}.end(), [](const auto& {pattern}) {{ return {cond}; }});"
            )
        }
        _ => anyhow::bail!("unsupported shape for C++"),
    };

    let indent: String = text[dec_start..]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    let replacement = format!("{indent}auto&& {range_name} = ({source});\n{indent}{statement}");

    Ok(PolyglotLoop {
        start: dec_start,
        end: close_brace + 1,
        indent,
        replacement,
        statement,
    })
}
