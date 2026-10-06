/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::syntax::{own_returns, return_value_end};

pub fn negate_expr_python(expr: &str) -> String {
    let t = expr.trim();
    if t == "True" {
        return "False".to_string();
    }
    if t == "False" {
        return "True".to_string();
    }
    if let Some(after) = t.strip_prefix("not ") {
        let trimmed = after.trim();
        if trimmed.starts_with('(')
            && trimmed.ends_with(')')
            && let Some(close) = crate::parameter_object::matching_bracket(trimmed, 0)
            && close == trimmed.len() - 1
        {
            return trimmed[1..trimmed.len() - 1].trim().to_string();
        }
        return trimmed.to_string();
    }
    if t.starts_with("not(")
        && t.ends_with(')')
        && let inside = &t[3..]
        && let Some(close) = crate::parameter_object::matching_bracket(inside, 0)
        && close == inside.len() - 1
    {
        return inside[1..inside.len() - 1].trim().to_string();
    }
    format!("not ({t})")
}

pub fn negate_expr_c_like(expr: &str) -> String {
    let t = expr.trim();
    if t == "true" {
        return "false".to_string();
    }
    if t == "false" {
        return "true".to_string();
    }
    if t.starts_with('!') && !t.starts_with("!=") && !t.starts_with("!==") {
        let after = t[1..].trim();
        if after.starts_with('(')
            && after.ends_with(')')
            && let Some(close) = crate::parameter_object::matching_bracket(after, 0)
            && close == after.len() - 1
        {
            return after[1..after.len() - 1].trim().to_string();
        }
        return after.to_string();
    }
    format!("!({t})")
}

pub fn negate_python_body(body: &str) -> String {
    let mut out_lines = Vec::new();
    let mut min_def_indent = None;

    for line in body.lines() {
        let trimmed = line.trim();
        let indent = line.len() - line.trim_start().len();

        if let Some(def_ind) = min_def_indent {
            if indent > def_ind {
                out_lines.push(line.to_string());
                continue;
            } else if !trimmed.is_empty() {
                min_def_indent = None;
            }
        }

        if trimmed.starts_with("def ") || trimmed.starts_with("async def ") {
            min_def_indent = Some(indent);
            out_lines.push(line.to_string());
            continue;
        }

        if trimmed.starts_with("return ") {
            let val = trimmed.strip_prefix("return ").unwrap().trim();
            let leading = &line[..indent];
            out_lines.push(format!("{leading}return {}", negate_expr_python(val)));
        } else {
            out_lines.push(line.to_string());
        }
    }
    out_lines.join("\n")
}

pub fn negate_c_like_body(body: &str) -> String {
    let mut out = body.to_string();
    let returns = own_returns(body);
    for at in returns.into_iter().rev() {
        let end = return_value_end(body, at);
        let val_with_semi = body[at + "return".len()..end].trim();
        let has_semi = val_with_semi.ends_with(';');
        let val = val_with_semi.trim_end_matches(';').trim();
        if !val.is_empty() {
            let neg = negate_expr_c_like(val);
            let semi = if has_semi { ";" } else { "" };
            out.replace_range(at..end, &format!("return {neg}{semi}"));
        }
    }
    let trimmed = out.trim();
    if !trimmed.contains(';') && !trimmed.contains("return") && !trimmed.is_empty() {
        return format!("\n    {}\n", negate_expr_c_like(trimmed));
    }
    out
}
