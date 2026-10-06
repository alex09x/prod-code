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

pub fn to_method_cpp(
    code: &str,
    target_class: Option<&str>,
    target_method: &str,
) -> Result<(String, String, String, String, String, usize, usize)> {
    let lines: Vec<&str> = code.lines().collect();
    let mut class_start = None;
    let mut class_end = None;
    let mut class_name = String::new();
    let mut brace_depth = 0i32;

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("class ") || trimmed.starts_with("struct ") {
            let words: Vec<&str> = trimmed.split_whitespace().collect();
            let mut name = "";
            for (w_idx, w) in words.iter().enumerate() {
                if (*w == "class" || *w == "struct") && w_idx + 1 < words.len() {
                    name = words[w_idx + 1]
                        .trim_matches(|c| c == '{' || c == ':')
                        .trim();
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
            brace_depth += line.chars().filter(|&c| c == '{').count() as i32;
            brace_depth -= line.chars().filter(|&c| c == '}').count() as i32;
            if brace_depth == 0 && line.contains('}') {
                class_end = Some(idx);
                break;
            }
        }
    }

    let c_start = class_start.context("Could not find class/struct in C++ file")?;
    let c_end = class_end.context("Could not find closing brace of C++ class")?;

    let mut method_line_idx = None;
    for (idx, line) in lines.iter().enumerate().take(c_end).skip(c_start + 1) {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with('*') {
            continue;
        }
        if let Some(paren_pos) = trimmed.find('(') {
            let before_paren = trimmed[..paren_pos].trim();
            let words: Vec<&str> = before_paren.split_whitespace().collect();
            if let Some(&last_word) = words.last() {
                let clean_name = last_word.trim_start_matches('*').trim_start_matches('&');
                if clean_name == target_method {
                    method_line_idx = Some(idx);
                    break;
                }
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
    let close_p = trimmed.find(')').context("Parameter list does not close")?;
    let params_str = &trimmed[open_p + 1..close_p];
    let params = split_call_arguments(params_str);
    let first_param = params
        .first()
        .context("Method takes no parameters; nothing can become receiver")?
        .clone();
    let is_const = first_param.starts_with("const ");
    let param_words: Vec<&str> = first_param.split_whitespace().collect();
    let param_name = param_words
        .last()
        .unwrap_or(&"")
        .trim_matches(|c| c == '&' || c == '*');

    let remaining_params = if params.len() > 1 {
        params[1..].join(", ")
    } else {
        String::new()
    };

    let without_static = trimmed.replace("static ", "");
    let const_suffix = if is_const && !without_static.contains(") const") {
        " const"
    } else {
        ""
    };
    let new_m_trimmed =
        if let (Some(op), Some(cp)) = (without_static.find('('), without_static.find(')')) {
            let after_cp = &without_static[cp + 1..];
            format!(
                "{}{remaining_params}){const_suffix}{after_cp}",
                &without_static[..op + 1]
            )
        } else {
            without_static
        };

    let mut m_body_end = m_idx;
    let mut m_depth = 0i32;
    let mut started = false;
    for (idx, line) in lines.iter().enumerate().take(c_end).skip(m_idx) {
        let opens = line.chars().filter(|&c| c == '{').count() as i32;
        let closes = line.chars().filter(|&c| c == '}').count() as i32;
        if opens > 0 {
            started = true;
        }
        m_depth += opens;
        m_depth -= closes;
        if started && m_depth == 0 {
            m_body_end = idx;
            break;
        }
    }

    let needle_param_dot = format!("{param_name}.");
    let needle_param_arrow = format!("{param_name}->");
    let mut renamed_uses = 0;
    let mut new_lines = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if idx == m_idx {
            new_lines.push(format!("{indent}{new_m_trimmed}"));
        } else if idx > m_idx && idx <= m_body_end {
            let mut cur = line.to_string();
            let mut search_idx = 0;
            while let Some(rel) = cur[search_idx..].find(&needle_param_dot) {
                let pos = search_idx + rel;
                if pos > 0 && is_ident(cur[..pos].chars().next_back().unwrap()) {
                    search_idx = pos + needle_param_dot.len();
                    continue;
                }
                cur.replace_range(pos..pos + needle_param_dot.len(), "this->");
                renamed_uses += 1;
                search_idx = pos + "this->".len();
            }
            search_idx = 0;
            while let Some(rel) = cur[search_idx..].find(&needle_param_arrow) {
                let pos = search_idx + rel;
                if pos > 0 && is_ident(cur[..pos].chars().next_back().unwrap()) {
                    search_idx = pos + needle_param_arrow.len();
                    continue;
                }
                cur.replace_range(pos..pos + needle_param_arrow.len(), "this->");
                renamed_uses += 1;
                search_idx = pos + "this->".len();
            }
            new_lines.push(cur);
        } else {
            new_lines.push(line.to_string());
        }
    }

    let intermediate = new_lines.join("\n");
    let (final_code, rewritten_calls) =
        rewrite_static_calls_in_code(&intermediate, target_method, &class_name, Language::Cpp);

    Ok((
        class_name,
        target_method.to_string(),
        first_param,
        "*this".to_string(),
        final_code,
        renamed_uses,
        rewritten_calls,
    ))
}
