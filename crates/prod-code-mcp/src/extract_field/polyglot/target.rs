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

use super::target_python::discover_python_target;
use crate::extract_field::helpers::is_ident;
use crate::extract_field::polyglot::parsers::{
    parse_params_cpp, parse_params_go, parse_params_swift, parse_params_ts,
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

pub(super) type TargetParts = (
    String,
    String,
    String,
    Vec<String>,
    usize,
    usize,
    usize,
    usize,
    Option<(usize, usize)>,
);

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
        Language::Python => discover_python_target(text, from, to)?,
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
