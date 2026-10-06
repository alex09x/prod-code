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
use crate::to_method::helpers::{is_ident, split_call_arguments};

pub fn to_method_py(
    code: &str,
    target_class: Option<&str>,
    target_method: &str,
) -> Result<(String, String, String, String, String, usize, usize)> {
    let lines: Vec<&str> = code.lines().collect();
    let mut class_start = None;
    let mut class_end = None;
    let mut class_name = String::new();
    let mut class_indent = 0;

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if let Some(after_class_raw) = trimmed.strip_prefix("class ") {
            let indent = line.len() - trimmed.len();
            let after_class = after_class_raw.trim_start();
            let name = after_class.split(['(', ':']).next().unwrap_or("").trim();
            if target_class.is_none() || target_class == Some(name) {
                class_start = Some(idx);
                class_name = name.to_string();
                class_indent = indent;
            }
        }
        if class_start.is_some() && idx > class_start.unwrap() && class_end.is_none() {
            let indent = line.len() - trimmed.len();
            if !trimmed.is_empty() && !trimmed.starts_with('#') && indent <= class_indent {
                class_end = Some(idx);
                break;
            }
        }
    }

    let c_start = class_start.context("Could not find class in Python file")?;
    let c_end = class_end.unwrap_or(lines.len());

    let mut method_line_idx = None;
    for (idx, line) in lines.iter().enumerate().take(c_end).skip(c_start + 1) {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            continue;
        }
        let def_needle = format!("def {target_method}(");
        let async_def_needle = format!("async def {target_method}(");
        if trimmed.starts_with(&def_needle) || trimmed.starts_with(&async_def_needle) {
            method_line_idx = Some(idx);
            break;
        }
    }

    let m_idx = method_line_idx
        .with_context(|| format!("Method `{target_method}` not found in class `{class_name}`"))?;

    let m_trimmed = lines[m_idx].trim_start();
    let open_p = m_trimmed.find('(').context("Missing parameter list")?;
    let close_p = m_trimmed
        .find(')')
        .context("Parameter list does not close")?;
    let params_str = &m_trimmed[open_p + 1..close_p];
    let params = split_call_arguments(params_str);
    let first_param = params
        .first()
        .context("Method takes no parameters; nothing can become `self`")?
        .clone();
    if first_param == "self" {
        anyhow::bail!("`{target_method}` already takes `self`");
    }
    let param_name = first_param
        .split(':')
        .next()
        .unwrap_or(&first_param)
        .trim()
        .to_string();

    let remaining_params = if params.len() > 1 {
        format!("self, {}", params[1..].join(", "))
    } else {
        "self".to_string()
    };

    let before_p = &m_trimmed[..open_p + 1];
    let after_p = &m_trimmed[close_p..];
    let indent_str = &lines[m_idx][..lines[m_idx].len() - m_trimmed.len()];
    let new_m_line = format!("{indent_str}{before_p}{remaining_params}{after_p}");

    let m_line = lines[m_idx];
    let m_indent = m_line.len() - m_line.trim_start().len();
    let mut m_body_end = m_idx;
    for (idx, line) in lines.iter().enumerate().take(c_end).skip(m_idx + 1) {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - trimmed.len();
        if indent <= m_indent {
            break;
        }
        m_body_end = idx;
    }

    let needle_param_dot = format!("{param_name}.");
    let mut renamed_uses = 0;
    let mut new_lines = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if m_idx > 0 && idx == m_idx - 1 && line.trim() == "@staticmethod" {
            continue;
        }
        if idx == m_idx {
            new_lines.push(new_m_line.clone());
        } else if idx > m_idx && idx <= m_body_end {
            let mut cur = line.to_string();
            let mut search_idx = 0;
            while let Some(rel) = cur[search_idx..].find(&needle_param_dot) {
                let pos = search_idx + rel;
                if pos > 0 && is_ident(cur[..pos].chars().next_back().unwrap()) {
                    search_idx = pos + needle_param_dot.len();
                    continue;
                }
                cur.replace_range(pos..pos + needle_param_dot.len(), "self.");
                renamed_uses += 1;
                search_idx = pos + "self.".len();
            }
            new_lines.push(cur);
        } else {
            new_lines.push(line.to_string());
        }
    }

    let intermediate = new_lines.join("\n");
    let (final_code, rewritten_calls) =
        rewrite_static_calls_in_code(&intermediate, target_method, &class_name, Language::Python);

    Ok((
        class_name,
        target_method.to_string(),
        first_param,
        "self".to_string(),
        final_code,
        renamed_uses,
        rewritten_calls,
    ))
}
