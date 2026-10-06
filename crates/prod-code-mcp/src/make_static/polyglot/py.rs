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

use crate::make_static::calls::rewrite_calls_in_code;
use crate::make_static::helpers::mentions;
use crate::make_static::types::Language;

pub fn make_static_py(
    code: &str,
    target_class: Option<&str>,
    target_method: &str,
    semantic_references: Option<&mut std::collections::HashSet<(u32, u32)>>,
) -> Result<(String, String, String, usize, Vec<String>)> {
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

    if m_idx > 0 && lines[m_idx - 1].trim() == "@staticmethod" {
        anyhow::bail!("`{target_method}` is already a static method");
    }

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

    let body_text = lines[m_idx + 1..=m_body_end].join("\n");
    if mentions(&body_text, "self") {
        anyhow::bail!(
            "`{target_method}` uses `self`; only a method that never accesses instance state can be made static"
        );
    }

    let m_trimmed = lines[m_idx].trim_start();
    let indent_str = &lines[m_idx][..lines[m_idx].len() - m_trimmed.len()];

    let modified_m_line = if let Some(open_p) = m_trimmed.find('(') {
        if let Some(close_p) = m_trimmed.find(')') {
            let before_p = &m_trimmed[..open_p + 1];
            let params = &m_trimmed[open_p + 1..close_p];
            let after_p = &m_trimmed[close_p..];
            let stripped_params = if let Some(rest) = params.trim().strip_prefix("self,") {
                rest.trim_start()
            } else if params.trim() == "self" {
                ""
            } else if let Some(rest) = params.trim().strip_prefix("self: ") {
                if let Some(comma) = rest.find(',') {
                    rest[comma + 1..].trim_start()
                } else {
                    ""
                }
            } else {
                params
            };
            format!("{indent_str}{before_p}{stripped_params}{after_p}")
        } else {
            lines[m_idx].to_string()
        }
    } else {
        lines[m_idx].to_string()
    };

    let mut new_lines = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if idx == m_idx {
            new_lines.push(format!("{indent_str}@staticmethod"));
            new_lines.push(modified_m_line.clone());
        } else {
            new_lines.push(line.to_string());
        }
    }

    let intermediate = new_lines.join("\n");
    let mut blocked = Vec::new();
    let mut semantic_references = semantic_references;
    if let Some(references) = semantic_references.as_deref_mut() {
        let declaration_line = u32::try_from(m_idx + 1).unwrap_or(u32::MAX);
        *references = references
            .drain()
            .map(|(line, col)| {
                if line >= declaration_line {
                    (line.saturating_add(1), col)
                } else {
                    (line, col)
                }
            })
            .collect();
    }
    let (final_code, rewritten_calls) = rewrite_calls_in_code(
        &intermediate,
        target_method,
        &class_name,
        Language::Python,
        "",
        &mut blocked,
        semantic_references,
    );

    Ok((
        class_name,
        target_method.to_string(),
        final_code,
        rewritten_calls,
        blocked,
    ))
}
