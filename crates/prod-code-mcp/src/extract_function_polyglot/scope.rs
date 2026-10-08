/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::HashSet;

use super::inference::infer_param_type;
use super::lang::{Language, mentions};
use super::tokenize::{is_builtin_or_global, is_keyword, tokenize_polyglot};
use super::types::{ExtractedParam, PolyTokenKind};

pub fn is_balanced(text: &str) -> bool {
    let mut stack = Vec::new();
    let mut in_str = None;
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if let Some(quote) = in_str {
            if c == '\\' {
                i += 2;
                continue;
            }
            if c == quote {
                in_str = None;
            }
        } else {
            match c {
                '"' | '\'' | '`' => in_str = Some(c),
                '(' | '[' | '{' => stack.push(c),
                ')' if stack.pop() != Some('(') => return false,
                ']' if stack.pop() != Some('[') => return false,
                '}' if stack.pop() != Some('{') => return false,
                _ => {}
            }
        }
        i += 1;
    }
    stack.is_empty() && in_str.is_none()
}

pub fn has_complete_expression_boundaries(text: &str, start: usize, end: usize) -> bool {
    let before = text[..start].trim_end();
    let left = before.chars().next_back();
    let left_ok = left.is_none_or(|c| matches!(c, '(' | '[' | '{' | ',' | ':' | '=' | ';'))
        || before.ends_with("return")
        || before.ends_with("=>");
    let after_slice = &text[end..];
    let horiz_ws = after_slice
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .map(|c| c.len_utf8())
        .sum::<usize>();
    let rest = &after_slice[horiz_ws..];
    let right = rest.chars().next();
    let right_ok = right
        .is_none_or(|c| matches!(c, ')' | ']' | '}' | ',' | ';' | ':' | '\n' | '\r'))
        || rest.starts_with("//")
        || rest.starts_with('#');
    left_ok && right_ok
}

pub fn find_enclosing_scope(
    text: &str,
    lang: Language,
    start: usize,
    _end: usize,
) -> (usize, usize, Option<String>, Option<String>, bool, String) {
    let mut enclosing_ret: Option<String> = None;
    let before = &text[..start];
    let mut scope_start = 0;
    let mut scope_end = text.len();
    let mut enclosing_fn = None;
    let mut is_method = false;
    let mut method_indent = String::new();

    match lang {
        Language::Python => {
            let sel_line = text[..start].rfind('\n').map_or(0, |i| i + 1);
            let sel_indent = text[sel_line..start]
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .count();

            for (pos, _) in before.rmatch_indices("def ") {
                let line_s = before[..pos].rfind('\n').map_or(0, |i| i + 1);
                let line_indent = before[line_s..pos]
                    .chars()
                    .take_while(|c| *c == ' ' || *c == '\t')
                    .count();
                if line_indent < sel_indent || sel_indent == 0 {
                    let head = &before[pos + 4..];
                    if let Some(open_p) = head.find('(') {
                        let fn_name = head[..open_p].trim().to_string();
                        enclosing_fn = Some(fn_name);
                        scope_start = line_s;
                        method_indent = " ".repeat(line_indent);
                        if line_indent > 0 {
                            is_method = true;
                        }
                    }
                    break;
                }
            }
            if scope_start > 0 {
                let def_indent = method_indent.len();
                let rest = &text[start..];
                let mut cur = start;
                for l in rest.lines() {
                    if l.trim().is_empty() || l.trim_start().starts_with('#') {
                        cur += l.len() + 1;
                        continue;
                    }
                    let cur_indent = l.chars().take_while(|c| *c == ' ' || *c == '\t').count();
                    if cur_indent <= def_indent && cur > start {
                        scope_end = cur;
                        break;
                    }
                    cur += l.len() + 1;
                }
            }
        }
        _ => {
            let mut cur = start;
            while let Some(b_open) = text[..cur].rfind('{') {
                if let Some(b_close) = crate::parameter_object::matching_bracket(text, b_open)
                    .filter(|&bc| b_open < start && start < bc)
                {
                    scope_start = text[..b_open].rfind('\n').map_or(0, |i| i + 1);
                    scope_end = b_close + 1;
                    let head = text[scope_start..b_open].trim();
                    if let Some((first_p, last_p)) = head
                        .rfind(')')
                        .and_then(|lp| head[..lp].rfind('(').map(|fp| (fp, lp)))
                    {
                        let before_p = head[..first_p].trim();
                        let fn_name = before_p.split_whitespace().last().unwrap_or("").to_string();
                        let is_control = matches!(
                            fn_name.as_str(),
                            "if" | "for"
                                | "while"
                                | "switch"
                                | "catch"
                                | "synchronized"
                                | "with"
                                | "lock"
                                | "using"
                                | "try"
                                | "else"
                                | "do"
                        );
                        if !fn_name.is_empty() && !is_control {
                            enclosing_fn = Some(fn_name);

                            let after_p = head[last_p + 1..].trim();
                            if after_p.starts_with("->") {
                                let clean = after_p.trim_start_matches("->").trim();
                                let ret_type = clean
                                    .split_whitespace()
                                    .next()
                                    .unwrap_or("")
                                    .trim_end_matches('{')
                                    .trim();
                                if !ret_type.is_empty() {
                                    enclosing_ret = Some(ret_type.to_string());
                                }
                            } else if after_p.starts_with(':') {
                                let clean = after_p.trim_start_matches(':').trim();
                                let ret_type = clean
                                    .split_whitespace()
                                    .next()
                                    .unwrap_or("")
                                    .trim_end_matches('{')
                                    .trim();
                                if !ret_type.is_empty() {
                                    enclosing_ret = Some(ret_type.to_string());
                                }
                            } else if !after_p.is_empty() && !after_p.starts_with('{') {
                                let ret_type = after_p
                                    .split_whitespace()
                                    .next()
                                    .unwrap_or("")
                                    .trim_end_matches('{')
                                    .trim();
                                if !ret_type.is_empty() {
                                    enclosing_ret = Some(ret_type.to_string());
                                }
                            } else if lang == Language::Java
                                || lang == Language::Cpp
                                || lang == Language::C
                                || lang == Language::Csharp
                            {
                                let parts: Vec<&str> = before_p.split_whitespace().collect();
                                if parts.len() >= 2 {
                                    let ty = parts[parts.len() - 2];
                                    if ty != "export"
                                        && ty != "static"
                                        && ty != "inline"
                                        && ty != "virtual"
                                        && ty != "fun"
                                        && ty != "def"
                                    {
                                        enclosing_ret = Some(ty.to_string());
                                    }
                                }
                            }
                            let indent_chars = text[scope_start..]
                                .chars()
                                .take_while(|c| *c == ' ' || *c == '\t')
                                .collect::<String>();
                            method_indent = indent_chars;
                            if !method_indent.is_empty() {
                                is_method = true;
                            }
                            break;
                        }
                    }
                }
                cur = b_open;
            }
        }
    }

    (
        scope_start,
        scope_end,
        enclosing_fn,
        enclosing_ret,
        is_method,
        method_indent,
    )
}

pub fn extract_input_variables(
    selection: &str,
    scope_before: &str,
    lang: Language,
) -> Vec<ExtractedParam> {
    let tokens = tokenize_polyglot(selection, lang);
    let mut declared_in_selection = HashSet::new();

    for (i, t) in tokens.iter().enumerate() {
        if matches!(t.text.as_str(), "let" | "const" | "var" | "val") && i + 1 < tokens.len() {
            let next = &tokens[i + 1];
            if next.kind == PolyTokenKind::Word && !is_keyword(&next.text, lang) {
                declared_in_selection.insert(next.text.clone());
            }
        } else if t.text == ":=" && i > 0 {
            let prev = &tokens[i - 1];
            if prev.kind == PolyTokenKind::Word && !is_keyword(&prev.text, lang) {
                declared_in_selection.insert(prev.text.clone());
            }
        } else if t.text == "=" && i > 0 && i + 1 < tokens.len() {
            let prev = &tokens[i - 1];
            if prev.kind == PolyTokenKind::Word
                && !is_keyword(&prev.text, lang)
                && (i == 1 || tokens[i - 2].text == "\n" || tokens[i - 2].text == ";")
            {
                declared_in_selection.insert(prev.text.clone());
            }
        }
    }

    let mut inputs = Vec::new();
    let mut seen = HashSet::new();

    for (i, t) in tokens.iter().enumerate() {
        if t.kind != PolyTokenKind::Word {
            continue;
        }
        let word = &t.text;
        if is_keyword(word, lang) || is_builtin_or_global(word, lang) {
            continue;
        }
        if i > 0 && (tokens[i - 1].text == "." || tokens[i - 1].text == "->") {
            continue;
        }
        if declared_in_selection.contains(word) {
            continue;
        }
        if mentions(scope_before, word) && seen.insert(word.clone()) {
            let ty = infer_param_type(word, scope_before, lang);
            inputs.push(ExtractedParam {
                name: word.clone(),
                ty,
            });
        }
    }

    inputs
}
