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

use crate::parameter_object::Language;

use super::header::{extract_polyglot_decl_name, is_polyglot_decl_header};

pub fn with_doc_comment_polyglot(text: &str, start: u32, lang: Language) -> u32 {
    let lines: Vec<&str> = text.lines().collect();
    let mut first = start;
    while first > 1 {
        let above = lines
            .get(first as usize - 2)
            .map(|l| l.trim())
            .unwrap_or("");
        if above.is_empty() {
            break;
        }
        let is_doc = match lang {
            Language::Python => above.starts_with('@') || above.starts_with('#'),
            Language::TypeScript | Language::JavaScript => {
                above.starts_with("//")
                    || above.starts_with("/*")
                    || above.starts_with('*')
                    || above.starts_with('@')
            }
            Language::Go => above.starts_with("//"),
            Language::Cpp | Language::C => {
                above.starts_with("//") || above.starts_with("/*") || above.starts_with('*')
            }
            Language::Swift => {
                above.starts_with("///")
                    || above.starts_with("//")
                    || above.starts_with("/*")
                    || above.starts_with('*')
                    || above.starts_with('@')
            }
            Language::Rust => {
                above.starts_with("///") || above.starts_with("#[") || above.starts_with("//!")
            }
            Language::Java => {
                above.starts_with("//")
                    || above.starts_with("/*")
                    || above.starts_with('*')
                    || above.starts_with('@')
            }
        };
        if is_doc {
            first -= 1;
        } else {
            break;
        }
    }
    first
}

pub fn find_matching_brace_end(lines: &[&str], start_line_idx: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut started = false;
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut in_backtick = false;
    let mut in_block_comment = false;

    for (idx, line) in lines.iter().enumerate().skip(start_line_idx) {
        let chars: Vec<char> = line.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            let next_c = chars.get(i + 1).copied();

            if in_block_comment {
                if c == '*' && next_c == Some('/') {
                    in_block_comment = false;
                    i += 2;
                    continue;
                }
                i += 1;
                continue;
            }

            if !in_single_quote && !in_double_quote && !in_backtick {
                if c == '/' && next_c == Some('/') {
                    break;
                }
                if c == '/' && next_c == Some('*') {
                    in_block_comment = true;
                    i += 2;
                    continue;
                }
            }

            if c == '\\' && (in_single_quote || in_double_quote || in_backtick) {
                i += 2;
                continue;
            }

            if c == '\x27' && !in_double_quote && !in_backtick {
                in_single_quote = !in_single_quote;
                i += 1;
                continue;
            }
            if c == '"' && !in_single_quote && !in_backtick {
                in_double_quote = !in_double_quote;
                i += 1;
                continue;
            }
            if c == '`' && !in_single_quote && !in_double_quote {
                in_backtick = !in_backtick;
                i += 1;
                continue;
            }

            if !in_single_quote && !in_double_quote && !in_backtick {
                if c == '{' {
                    depth += 1;
                    started = true;
                } else if c == '}' {
                    depth -= 1;
                    if started && depth <= 0 {
                        return Some(idx);
                    }
                }
            }

            i += 1;
        }
    }
    None
}

pub fn find_polyglot_decl(text: &str, line: u32, lang: Language) -> Result<(String, u32, u32)> {
    let lines: Vec<&str> = text.lines().collect();
    if line == 0 || line as usize > lines.len() {
        anyhow::bail!("line {line} is out of bounds (1..={})", lines.len());
    }

    let target_idx = (line - 1) as usize;

    let mut header_idx = target_idx;
    let trimmed_target = lines[target_idx].trim();
    if trimmed_target.is_empty()
        || trimmed_target.starts_with("//")
        || trimmed_target.starts_with("/*")
        || trimmed_target.starts_with('*')
        || trimmed_target.starts_with('@')
        || (lang == Language::Python && trimmed_target.starts_with('#'))
    {
        for (i, line_str) in lines
            .iter()
            .enumerate()
            .take(target_idx + 20)
            .skip(target_idx + 1)
        {
            let t = line_str.trim();
            if t.is_empty()
                || t.starts_with("//")
                || t.starts_with("/*")
                || t.starts_with('*')
                || t.starts_with('@')
                || (lang == Language::Python && t.starts_with('#'))
            {
                continue;
            }
            if is_polyglot_decl_header(line_str, lang) {
                header_idx = i;
                break;
            }
        }
    }

    if !is_polyglot_decl_header(lines[header_idx], lang) {
        let mut found = None;
        for i in (0..=target_idx).rev() {
            if is_polyglot_decl_header(lines[i], lang) {
                found = Some(i);
                break;
            }
        }
        header_idx = found.with_context(|| format!("no declaration found at line {line}"))?;
    }

    if lang == Language::Go && lines[header_idx].trim().starts_with("func (") {
        let name = extract_polyglot_decl_name(lines[header_idx], lang).unwrap_or_default();
        anyhow::bail!("`{name}` is a method with a receiver; move it with `code_move_method`");
    }

    let decl_name = extract_polyglot_decl_name(lines[header_idx], lang).with_context(|| {
        format!(
            "could not parse declaration name at line {}",
            header_idx + 1
        )
    })?;

    let start_line = with_doc_comment_polyglot(text, (header_idx + 1) as u32, lang);

    let end_line = match lang {
        Language::Python => {
            let mut end_idx = header_idx;
            for (i, line_str) in lines.iter().enumerate().skip(header_idx + 1) {
                let trimmed = line_str.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    end_idx = i;
                    continue;
                }
                let indent = line_str.len() - line_str.trim_start().len();
                if indent == 0 {
                    break;
                }
                end_idx = i;
            }
            (end_idx + 1) as u32
        }
        _ => {
            if let Some(brace_end_idx) = find_matching_brace_end(&lines, header_idx) {
                let mut e = brace_end_idx;
                if e + 1 < lines.len() && lines[e + 1].trim() == ";" {
                    e += 1;
                }
                (e + 1) as u32
            } else {
                let mut semi_end_idx = header_idx;
                for (i, line_str) in lines.iter().enumerate().skip(header_idx) {
                    semi_end_idx = i;
                    if line_str.contains(';') {
                        break;
                    }
                }
                (semi_end_idx + 1) as u32
            }
        }
    };

    Ok((decl_name, start_line, end_line))
}
