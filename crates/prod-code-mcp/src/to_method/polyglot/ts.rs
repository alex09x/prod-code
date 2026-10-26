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

pub fn to_method_ts(
    code: &str,
    target_class: Option<&str>,
    target_method: &str,
) -> Result<(String, String, String, String, String, usize, usize)> {
    let lines: Vec<&str> = code.lines().collect();
    let mut class_start = None;
    let mut class_end = None;
    let mut class_name = String::new();
    let mut brace_depth = 0i32;
    let syntax = super::binding::mask(code, Language::TypeScript);
    let mut offset = 0;

    for (idx, line) in lines.iter().enumerate() {
        let line_offset = offset;
        offset += line.len()
            + if code.as_bytes().get(line_offset + line.len()) == Some(&b'\r') {
                2
            } else {
                1
            };
        let trimmed = line.trim_start();
        if trimmed.starts_with("class ")
            || trimmed.starts_with("export class ")
            || trimmed.starts_with("export default class ")
        {
            let words: Vec<&str> = trimmed.split_whitespace().collect();
            let mut name = "";
            for (w_idx, w) in words.iter().enumerate() {
                if *w == "class" && w_idx + 1 < words.len() {
                    name = words[w_idx + 1].trim_matches('{').trim();
                    break;
                }
            }
            if target_class.is_none() || target_class == Some(name) {
                class_start = Some(idx);
                class_name = name.to_string();
                brace_depth = 0;
            }
        }
        if class_start.is_some() && class_end.is_none() {
            brace_depth += line
                .bytes()
                .enumerate()
                .filter(|(i, c)| *c == b'{' && syntax[line_offset + i])
                .count() as i32;
            brace_depth -= line
                .bytes()
                .enumerate()
                .filter(|(i, c)| *c == b'}' && syntax[line_offset + i])
                .count() as i32;
            if brace_depth == 0 && line.contains('}') {
                class_end = Some(idx);
                break;
            }
        }
    }

    let c_start = class_start.context("Could not find class in TypeScript/JavaScript file")?;
    let c_end = class_end.context("Could not find closing brace of class")?;

    let mut method_line_idx = None;
    for (idx, line) in lines.iter().enumerate().take(c_end).skip(c_start + 1) {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with('*') {
            continue;
        }
        if let Some(paren_pos) = trimmed.find('(') {
            let before_paren = trimmed[..paren_pos].trim();
            let words: Vec<&str> = before_paren.split_whitespace().collect();
            if words.last() == Some(&target_method) {
                method_line_idx = Some(idx);
                break;
            }
        }
    }

    let m_idx = method_line_idx
        .with_context(|| format!("Method `{target_method}` not found in class `{class_name}`"))?;

    let m_line = lines[m_idx];
    if !m_line.contains("static ") {
        anyhow::bail!("`{target_method}` is already an instance method");
    }

    let trimmed = m_line.trim_start();
    let indent = &m_line[..m_line.len() - trimmed.len()];
    let open_p = trimmed.find('(').context("Missing parameter list")?;
    let close_p = super::binding::parameter_close(trimmed, Language::TypeScript)?;
    let params_str = &trimmed[open_p + 1..close_p];
    let params = split_call_arguments(params_str);
    let first_param = params
        .first()
        .context("Method takes no parameters; nothing can become `this`")?
        .clone();
    let param_name = first_param
        .split(':')
        .next()
        .unwrap_or(&first_param)
        .trim()
        .to_string();

    let remaining_params = if params.len() > 1 {
        params[1..].join(", ")
    } else {
        String::new()
    };

    super::binding::parameters(
        &first_param,
        &param_name,
        &remaining_params,
        Language::TypeScript,
    )?;
    let without_static = trimmed.replacen("static ", "", 1);
    let new_m_trimmed = if let Some(op) = without_static.find('(') {
        let cp = super::binding::parameter_close(&without_static, Language::TypeScript)?;
        format!(
            "{}{remaining_params}{}",
            &without_static[..op + 1],
            &without_static[cp..]
        )
    } else {
        without_static
    };

    let (intermediate, renamed_uses) = super::body::braced(
        code,
        m_idx,
        &format!("{indent}{new_m_trimmed}"),
        &param_name,
        &first_param,
        Language::TypeScript,
    )?;
    let (final_code, rewritten_calls) = rewrite_static_calls_in_code(
        &intermediate,
        target_method,
        &class_name,
        Language::TypeScript,
    );

    Ok((
        class_name,
        target_method.to_string(),
        first_param,
        "this".to_string(),
        final_code,
        renamed_uses,
        rewritten_calls,
    ))
}
