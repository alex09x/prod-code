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

use super::rewrite::rewrite_static_calls_in_code;
use crate::make_static::Language;
use crate::to_method::helpers::split_call_arguments;

pub fn to_method_go(
    code: &str,
    target_struct: Option<&str>,
    target_func: &str,
) -> Result<(String, String, String, String, String, usize, usize)> {
    let lines: Vec<&str> = code.lines().collect();
    let mut func_line_idx = None;
    let needle_func = format!("func {target_func}(");

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with(&needle_func) {
            func_line_idx = Some(idx);
            break;
        }
    }

    let f_idx =
        func_line_idx.with_context(|| format!("Function `{target_func}` not found in Go file"))?;

    let f_line = lines[f_idx];
    let trimmed = f_line.trim_start();
    let indent = &f_line[..f_line.len() - trimmed.len()];
    let open_p = trimmed.find('(').context("Missing parameter list")?;
    let close_p = trimmed.find(')').context("Parameter list does not close")?;
    let params_str = &trimmed[open_p + 1..close_p];
    let params = split_call_arguments(params_str);
    let first_param = params
        .first()
        .context("Function takes no parameters; nothing can become receiver")?
        .clone();

    let param_words: Vec<&str> = first_param.split_whitespace().collect();
    if param_words.len() < 2 {
        anyhow::bail!("First parameter `{first_param}` has no type");
    }
    let s_type = param_words[1];
    let s_name = s_type.trim_start_matches('*');
    if let Some(target) = target_struct
        && target != s_name
    {
        anyhow::bail!("First parameter type `{s_name}` does not match target struct `{target}`");
    }

    let remaining_params = if params.len() > 1 {
        params[1..].join(", ")
    } else {
        String::new()
    };

    let after_cp = &trimmed[close_p + 1..];
    let new_f_line =
        format!("{indent}func ({first_param}) {target_func}({remaining_params}){after_cp}");

    let mut new_lines = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if idx == f_idx {
            new_lines.push(new_f_line.clone());
        } else {
            new_lines.push(line.to_string());
        }
    }

    let intermediate = new_lines.join("\n");
    let (final_code, rewritten_calls) =
        rewrite_static_calls_in_code(&intermediate, target_func, s_name, Language::Go);

    Ok((
        s_name.to_string(),
        target_func.to_string(),
        first_param.clone(),
        format!("({first_param})"),
        final_code,
        0,
        rewritten_calls,
    ))
}
