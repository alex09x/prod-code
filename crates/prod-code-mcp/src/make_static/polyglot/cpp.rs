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

pub fn make_static_cpp(
    code: &str,
    target_class: Option<&str>,
    target_method: &str,
    semantic_references: Option<&mut std::collections::HashSet<(u32, u32)>>,
) -> Result<(String, String, String, usize, Vec<String>)> {
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
    if m_line.trim_start().starts_with("static ") {
        anyhow::bail!("`{target_method}` is already static");
    }

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

    let body_text = lines[m_idx..=m_body_end].join("\n");
    if mentions(&body_text, "this") {
        anyhow::bail!(
            "`{target_method}` uses `this`; only a method that never accesses instance state can be made static"
        );
    }

    let mut new_lines = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if idx == m_idx {
            let trimmed = line.trim_start();
            let indent = &line[..line.len() - trimmed.len()];
            let without_const = if let Some(pos) = trimmed.find(") const") {
                let before = &trimmed[..pos + 1];
                let after = &trimmed[pos + 7..];
                format!("{before}{after}")
            } else {
                trimmed.to_string()
            };
            new_lines.push(format!("{indent}static {without_const}"));
        } else {
            new_lines.push(line.to_string());
        }
    }

    let intermediate = new_lines.join("\n");
    let mut blocked = Vec::new();
    let (final_code, rewritten_calls) = rewrite_calls_in_code(
        &intermediate,
        target_method,
        &class_name,
        Language::Cpp,
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
