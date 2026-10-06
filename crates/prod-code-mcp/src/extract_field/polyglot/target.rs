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

use crate::extract_field::helpers::is_ident;
use crate::extract_field::polyglot::parsers::{
    parse_params_cpp, parse_params_go, parse_params_python, parse_params_swift, parse_params_ts,
};
use crate::parameter_object::Language;

pub struct PolyglotTarget {
    pub owner: String,
    pub method: String,
    pub receiver_name: String,
    pub params: Vec<String>,
    pub body_open: usize,
    pub body_close: usize,
    pub class_body_open: usize,
    pub class_close_line_start: usize,
    pub init_body: Option<(usize, usize)>,
    pub is_static_method: bool,
}

pub fn discover_target(
    text: &str,
    lang: Language,
    from: usize,
    to: usize,
) -> Result<PolyglotTarget> {
    let mut is_static_method = false;
    let (
        owner,
        method,
        receiver_name,
        params,
        body_open,
        body_close,
        class_body_open,
        class_close_line_start,
        init_body,
    ) = match lang {
        Language::Go => {
            let mut found = None;
            for (pos, _) in text.match_indices("func ") {
                let after = &text[pos + 5..];
                if !after.starts_with('(') {
                    continue;
                }
                let Some(recv_close) = after.find(')') else {
                    continue;
                };
                let recv_slice = after[1..recv_close].trim();
                let parts: Vec<&str> = recv_slice.split_whitespace().collect();
                if parts.len() < 2 {
                    continue;
                }
                let r_name = parts[0];
                let o_name = parts[1].trim_start_matches('*').trim_start_matches('&');
                let after_recv = &after[recv_close + 1..];
                let Some(m_paren) = after_recv.find('(') else {
                    continue;
                };
                let m_name = after_recv[..m_paren].trim();
                let Some(p_close) = crate::parameter_object::matching_bracket(after_recv, m_paren)
                else {
                    continue;
                };
                let p_slice = &after_recv[m_paren + 1..p_close];
                let Some(b_open_rel) = after_recv[p_close..].find('{') else {
                    continue;
                };
                let b_open = pos + 5 + recv_close + 1 + p_close + b_open_rel;
                let Some(b_close) = crate::parameter_object::matching_bracket(text, b_open) else {
                    continue;
                };
                if b_open < from && to <= b_close {
                    found = Some((
                        o_name.to_string(),
                        m_name.to_string(),
                        r_name.to_string(),
                        parse_params_go(p_slice),
                        b_open,
                        b_close,
                    ));
                    break;
                }
            }
            let (o_name, m_name, r_name, p_list, b_open, b_close) =
                found.context("selection is not inside a Go method with a receiver")?;

            let needle = format!("type {o_name} struct");
            let struct_at = text
                .find(&needle)
                .with_context(|| format!("cannot find struct declaration for `{o_name}`"))?;
            let s_open = text[struct_at..]
                .find('{')
                .map(|i| struct_at + i)
                .context("struct has no `{`")?;
            let s_close = crate::parameter_object::matching_bracket(text, s_open)
                .context("struct `{` does not close")?;
            let s_close_line_start = text[..s_close].rfind('\n').map_or(0, |i| i + 1);

            (
                o_name,
                m_name,
                r_name,
                p_list,
                b_open,
                b_close,
                s_open,
                s_close_line_start,
                None,
            )
        }
        Language::Python => {
            let lines: Vec<&str> = text.lines().collect();
            let mut line_offsets = Vec::new();
            let mut off = 0;
            for l in &lines {
                line_offsets.push(off);
                off += l.len() + 1;
            }
            let sel_line_idx = lines
                .iter()
                .enumerate()
                .position(|(idx, _)| {
                    let start = line_offsets[idx];
                    let end = start + lines[idx].len();
                    start <= from && from <= end
                })
                .context("cannot find line of selection")?;

            let mut def_line_idx = None;
            for idx in (0..=sel_line_idx).rev() {
                let trimmed = lines[idx].trim_start();
                if trimmed.starts_with("def ") || trimmed.starts_with("async def ") {
                    def_line_idx = Some(idx);
                    break;
                }
            }
            let def_idx =
                def_line_idx.context("selection is not inside a Python function/method")?;
            let def_line = lines[def_idx];
            let def_indent = def_line.len() - def_line.trim_start().len();
            let def_head = def_line
                .trim_start()
                .strip_prefix("async ")
                .unwrap_or(def_line.trim_start());
            let m_name = def_head
                .strip_prefix("def ")
                .unwrap()
                .split('(')
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
            let p_start = def_line.find('(').context("method has no `(`")?;
            let p_end = def_line.rfind(')').context("method has no `)`")?;
            let p_slice = &def_line[p_start + 1..p_end];
            let p_list = parse_params_python(p_slice);
            anyhow::ensure!(
                def_line[p_start + 1..p_end].trim().starts_with("self"),
                "`{m_name}` takes no `self`, so it has no field to read; extract a parameter instead"
            );

            let b_open =
                line_offsets[def_idx] + def_line.find(':').context("def line has no `:`")? + 1;
            let mut b_close = text.len();
            for idx in def_idx + 1..lines.len() {
                let l = lines[idx];
                if l.trim().is_empty() || l.trim_start().starts_with('#') {
                    continue;
                }
                let ind = l.len() - l.trim_start().len();
                if ind <= def_indent {
                    b_close = line_offsets[idx];
                    break;
                }
            }
            anyhow::ensure!(
                b_open < from && to <= b_close,
                "selection runs past the end of `{m_name}`"
            );

            let mut class_line_idx = None;
            for idx in (0..def_idx).rev() {
                let trimmed = lines[idx].trim_start();
                if trimmed.starts_with("class ") {
                    let ind = lines[idx].len() - trimmed.len();
                    if ind < def_indent {
                        class_line_idx = Some(idx);
                        break;
                    }
                }
            }
            let c_idx = class_line_idx.context("method is not inside a class")?;
            let c_line = lines[c_idx];
            let o_name = c_line
                .trim_start()
                .strip_prefix("class ")
                .unwrap()
                .split(['(', ':'])
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
            let c_colon =
                line_offsets[c_idx] + c_line.find(':').context("class line has no `:`")? + 1;

            let mut init_info = None;
            for idx in c_idx + 1..lines.len() {
                let l = lines[idx];
                let trimmed = l.trim_start();
                let ind = l.len() - trimmed.len();
                if !trimmed.is_empty()
                    && !trimmed.starts_with('#')
                    && ind <= (c_line.len() - c_line.trim_start().len())
                {
                    break;
                }
                if trimmed.starts_with("def __init__(") {
                    let init_indent = ind;
                    let init_start = line_offsets[idx] + l.find(':').unwrap_or(0) + 1;
                    let mut init_end = line_offsets[idx] + l.len();
                    for j in idx + 1..lines.len() {
                        let jl = lines[j];
                        if jl.trim().is_empty() || jl.trim_start().starts_with('#') {
                            continue;
                        }
                        let jind = jl.len() - jl.trim_start().len();
                        if jind <= init_indent {
                            init_end = line_offsets[j];
                            break;
                        }
                        init_end = line_offsets[j] + jl.len();
                    }
                    init_info = Some((init_start, init_end));
                    break;
                }
            }

            (
                o_name,
                m_name,
                "self".to_string(),
                p_list,
                b_open,
                b_close,
                c_colon,
                c_colon,
                init_info,
            )
        }
        Language::TypeScript | Language::JavaScript => {
            let mut found_class = None;
            for (pos, _) in text.match_indices("class ") {
                if pos > 0 && is_ident(text[..pos].chars().next_back().unwrap()) {
                    continue;
                }
                let after = &text[pos + 6..];
                let o_name = after
                    .split(['{', ' ', '\n', '<'])
                    .next()
                    .unwrap_or("")
                    .trim();
                let Some(c_open) = text[pos..].find('{').map(|i| pos + i) else {
                    continue;
                };
                let Some(c_close) = crate::parameter_object::matching_bracket(text, c_open) else {
                    continue;
                };
                if c_open < from && to <= c_close {
                    found_class = Some((o_name.to_string(), c_open, c_close));
                    break;
                }
            }
            let (o_name, c_open, c_close) =
                found_class.context("selection is not inside a class")?;

            let mut found_method = None;
            let class_inner = &text[c_open + 1..c_close];
            let mut cur = 0;
            while let Some(rel_open) = class_inner[cur..].find('{') {
                let b_open = c_open + 1 + cur + rel_open;
                cur += rel_open + 1;
                let Some(b_close) = crate::parameter_object::matching_bracket(text, b_open) else {
                    continue;
                };
                if b_open < from && to <= b_close {
                    let before_body = text[c_open + 1..b_open].trim_end();
                    let last_paren = before_body.rfind(')').context("method has no `)`")?;
                    let first_paren = before_body[..last_paren]
                        .rfind('(')
                        .context("method has no `(`")?;
                    let p_slice = &before_body[first_paren + 1..last_paren];
                    let head = before_body[..first_paren].trim_end();
                    let m_name = head.split_whitespace().last().unwrap_or("").trim();
                    is_static_method = head.split_whitespace().any(|word| word == "static");
                    found_method = Some((
                        m_name.to_string(),
                        parse_params_ts(p_slice),
                        b_open,
                        b_close,
                    ));
                    break;
                }
            }
            let (m_name, p_list, b_open, b_close) =
                found_method.context("selection is not inside a method")?;
            (
                o_name,
                m_name,
                "this".to_string(),
                p_list,
                b_open,
                b_close,
                c_open,
                c_close,
                None,
            )
        }
        Language::Swift => {
            let mut found_type = None;
            for kind in ["class ", "struct ", "actor "] {
                for (pos, _) in text.match_indices(kind) {
                    if pos > 0 && is_ident(text[..pos].chars().next_back().unwrap()) {
                        continue;
                    }
                    let after = &text[pos + kind.len()..];
                    let o_name = after
                        .split(['{', ' ', '\n', ':', '<'])
                        .next()
                        .unwrap_or("")
                        .trim();
                    let Some(t_open) = text[pos..].find('{').map(|i| pos + i) else {
                        continue;
                    };
                    let Some(t_close) = crate::parameter_object::matching_bracket(text, t_open)
                    else {
                        continue;
                    };
                    if t_open < from && to <= t_close {
                        found_type = Some((o_name.to_string(), t_open, t_close));
                        break;
                    }
                }
                if found_type.is_some() {
                    break;
                }
            }
            let (o_name, t_open, t_close) =
                found_type.context("selection is not inside a class or struct")?;

            let mut found_func = None;
            let type_inner = &text[t_open + 1..t_close];
            for (pos, _) in type_inner.match_indices("func ") {
                let func_abs = t_open + 1 + pos;
                if func_abs > 0 && is_ident(text[..func_abs].chars().next_back().unwrap()) {
                    continue;
                }
                let after = &text[func_abs + 5..];
                let Some(p_open) = after.find('(') else {
                    continue;
                };
                let m_name = after[..p_open].trim();
                let Some(p_close) = crate::parameter_object::matching_bracket(after, p_open) else {
                    continue;
                };
                let p_slice = &after[p_open + 1..p_close];
                let Some(b_open_rel) = after[p_close..].find('{') else {
                    continue;
                };
                let b_open = func_abs + 5 + p_close + b_open_rel;
                let Some(b_close) = crate::parameter_object::matching_bracket(text, b_open) else {
                    continue;
                };
                if b_open < from && to <= b_close {
                    found_func = Some((
                        m_name.to_string(),
                        parse_params_swift(p_slice),
                        b_open,
                        b_close,
                    ));
                    break;
                }
            }
            let (m_name, p_list, b_open, b_close) =
                found_func.context("selection is not inside a Swift method")?;
            (
                o_name,
                m_name,
                "self".to_string(),
                p_list,
                b_open,
                b_close,
                t_open,
                t_close,
                None,
            )
        }
        Language::Cpp | Language::C => {
            let mut found_type = None;
            for kind in ["class ", "struct "] {
                for (pos, _) in text.match_indices(kind) {
                    if pos > 0 && is_ident(text[..pos].chars().next_back().unwrap()) {
                        continue;
                    }
                    let after = &text[pos + kind.len()..];
                    let o_name = after
                        .split(['{', ' ', '\n', ':', ';'])
                        .next()
                        .unwrap_or("")
                        .trim();
                    let Some(t_open) = text[pos..].find('{').map(|i| pos + i) else {
                        continue;
                    };
                    let Some(t_close) = crate::parameter_object::matching_bracket(text, t_open)
                    else {
                        continue;
                    };
                    if t_open < from && to <= t_close {
                        found_type = Some((o_name.to_string(), t_open, t_close));
                        break;
                    }
                }
                if found_type.is_some() {
                    break;
                }
            }
            let (o_name, t_open, t_close) =
                found_type.context("selection is not inside a C++ class or struct")?;

            let mut found_method = None;
            let class_inner = &text[t_open + 1..t_close];
            let mut cur = 0;
            while let Some(rel_open) = class_inner[cur..].find('{') {
                let b_open = t_open + 1 + cur + rel_open;
                cur += rel_open + 1;
                let Some(b_close) = crate::parameter_object::matching_bracket(text, b_open) else {
                    continue;
                };
                if b_open < from && to <= b_close {
                    let before_body = text[t_open + 1..b_open].trim_end();
                    let last_paren = before_body.rfind(')').context("method has no `)`")?;
                    let first_paren = before_body[..last_paren]
                        .rfind('(')
                        .context("method has no `(`")?;
                    let p_slice = &before_body[first_paren + 1..last_paren];
                    let head = before_body[..first_paren].trim_end();
                    let m_name = head
                        .split_whitespace()
                        .last()
                        .unwrap_or("")
                        .trim()
                        .trim_start_matches('*')
                        .trim_start_matches('&');
                    found_method = Some((
                        m_name.to_string(),
                        parse_params_cpp(p_slice),
                        b_open,
                        b_close,
                    ));
                    break;
                }
            }
            let (m_name, p_list, b_open, b_close) =
                found_method.context("selection is not inside a C++ method")?;
            let t_close_line_start = text[..t_close].rfind('\n').map_or(0, |i| i + 1);
            (
                o_name,
                m_name,
                "this".to_string(),
                p_list,
                b_open,
                b_close,
                t_open,
                t_close_line_start,
                None,
            )
        }
        Language::Rust | Language::Java => unreachable!(),
    };

    Ok(PolyglotTarget {
        owner,
        method,
        receiver_name,
        params,
        body_open,
        body_close,
        class_body_open,
        class_close_line_start,
        init_body,
        is_static_method,
    })
}
