/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::target::TargetParts;
use crate::extract_field::polyglot::parsers::parse_params_python;
use anyhow::{Context, Result};

pub(super) fn discover_python_target(text: &str, from: usize, to: usize) -> Result<TargetParts> {
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
    let def_idx = def_line_idx.context("selection is not inside a Python function/method")?;
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
    let b_open = line_offsets[def_idx] + def_line.find(':').context("def line has no `:`")? + 1;
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
    let c_colon = line_offsets[c_idx] + c_line.find(':').context("class line has no `:`")? + 1;
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
    Ok((
        o_name,
        m_name,
        "self".to_string(),
        p_list,
        b_open,
        b_close,
        c_colon,
        c_colon,
        init_info,
    ))
}
