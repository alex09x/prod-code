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

pub fn make_static_go(
    code: &str,
    target_struct: Option<&str>,
    target_method: &str,
    semantic_references: Option<&mut std::collections::HashSet<(u32, u32)>>,
) -> Result<(String, String, String, usize, Vec<String>)> {
    let lines: Vec<&str> = code.lines().collect();
    let mut method_line_idx = None;
    let mut receiver_name = String::new();
    let mut struct_name = String::new();

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("func (") {
            continue;
        }
        let after_func = &trimmed["func (".len()..];
        let Some(close_recv) = after_func.find(')') else {
            continue;
        };
        let recv_part = after_func[..close_recv].trim();
        let after_recv = after_func[close_recv + 1..].trim_start();
        let Some(open_p) = after_recv.find('(') else {
            continue;
        };
        let m_name = after_recv[..open_p].trim();
        if m_name == target_method {
            let recv_words: Vec<&str> = recv_part.split_whitespace().collect();
            if recv_words.len() >= 2 {
                let r_name = recv_words[0];
                let s_name = recv_words[1].trim_start_matches('*');
                if target_struct.is_none() || target_struct == Some(s_name) {
                    method_line_idx = Some(idx);
                    receiver_name = r_name.to_string();
                    struct_name = s_name.to_string();
                    break;
                }
            }
        }
    }

    let m_idx = method_line_idx
        .with_context(|| format!("Method `{target_method}` with receiver not found in Go file"))?;

    let mut m_body_end = m_idx;
    let mut m_depth = 0i32;
    let mut started = false;
    for (idx, line) in lines.iter().enumerate().skip(m_idx) {
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

    let mut body_parts = Vec::new();
    if let Some(pos) = lines[m_idx].find('{') {
        let after_brace = &lines[m_idx][pos + 1..];
        if !after_brace.trim().is_empty() {
            body_parts.push(after_brace);
        }
    }
    if m_body_end > m_idx {
        for line in &lines[m_idx + 1..m_body_end] {
            body_parts.push(*line);
        }
        if let Some(pos) = lines[m_body_end].rfind('}') {
            let before_brace = &lines[m_body_end][..pos];
            if !before_brace.trim().is_empty() {
                body_parts.push(before_brace);
            }
        } else {
            body_parts.push(lines[m_body_end]);
        }
    }
    let body_text = body_parts.join("\n");
    if mentions(&body_text, &receiver_name) {
        anyhow::bail!(
            "`{target_method}` uses receiver `{receiver_name}`; only a method that never accesses its receiver can be made static"
        );
    }

    let mut new_lines = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if idx == m_idx {
            let trimmed = line.trim_start();
            let indent = &line[..line.len() - trimmed.len()];
            let after_func = &trimmed["func (".len()..];
            let close_recv = after_func.find(')').unwrap();
            let after_recv = after_func[close_recv + 1..].trim_start();
            new_lines.push(format!("{indent}func {after_recv}"));
        } else {
            new_lines.push(line.to_string());
        }
    }

    let intermediate = new_lines.join("\n");
    let mut blocked = Vec::new();
    let (final_code, rewritten_calls) = rewrite_calls_in_code(
        &intermediate,
        target_method,
        &struct_name,
        Language::Go,
        "",
        &mut blocked,
        semantic_references,
    );

    Ok((
        struct_name,
        target_method.to_string(),
        final_code,
        rewritten_calls,
        blocked,
    ))
}
