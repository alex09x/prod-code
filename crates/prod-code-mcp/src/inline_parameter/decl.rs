/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::parameter_object::Language;
use anyhow::{Context, Result};

use super::syntax::is_ident;
use super::types::PolyglotDecl;

pub fn extract_decl_name_from_line(line: &str, lang: Language) -> Option<String> {
    let trimmed = line.trim();
    match lang {
        Language::Python => {
            let rest = trimmed.strip_prefix("async ").unwrap_or(trimmed);
            if let Some(after_def) = rest.strip_prefix("def ") {
                let paren = after_def.find('(')?;
                let name = after_def[..paren].trim();
                return Some(name.to_string());
            }
        }
        Language::Go => {
            if let Some(after_func) = trimmed.strip_prefix("func ") {
                if after_func.starts_with('(') {
                    let close_recv = after_func.find(')')?;
                    let after_recv = after_func[close_recv + 1..].trim_start();
                    let paren = after_recv.find('(')?;
                    let name = after_recv[..paren].trim();
                    return Some(name.to_string());
                } else if let Some(paren) = after_func.find('(') {
                    let name = after_func[..paren].trim();
                    return Some(name.to_string());
                }
            }
        }
        Language::Swift => {
            if let Some(pos) = trimmed.find("func ") {
                let after = &trimmed[pos + 5..];
                let paren = after.find('(').or_else(|| after.find('<'))?;
                let name = after[..paren].trim();
                return Some(name.to_string());
            }
        }
        Language::TypeScript | Language::JavaScript => {
            if let Some(pos) = trimmed.find("function ") {
                let after = &trimmed[pos + 9..];
                let paren = after.find('(').or_else(|| after.find('<'))?;
                let name = after[..paren].trim();
                return Some(name.to_string());
            }
            if let Some(paren) = trimmed.find('(') {
                let before = trimmed[..paren].trim();
                if let Some(name) = before.split_whitespace().last() {
                    let clean = name.trim_end_matches('<');
                    if clean.chars().all(is_ident)
                        && !clean.is_empty()
                        && clean != "if"
                        && clean != "while"
                        && clean != "for"
                        && clean != "switch"
                    {
                        return Some(clean.to_string());
                    }
                }
            }
        }
        Language::Cpp | Language::C | Language::Java => {
            if let Some(paren) = trimmed.find('(') {
                let before = trimmed[..paren].trim();
                if let Some(name) = before.split_whitespace().last() {
                    let clean = name.trim_start_matches('*').trim_start_matches('&');
                    let member = clean.rsplit("::").next().unwrap_or(clean);
                    if member.chars().all(is_ident)
                        && !member.is_empty()
                        && member != "if"
                        && member != "while"
                        && member != "for"
                        && member != "switch"
                    {
                        return Some(member.to_string());
                    }
                }
            }
        }
        Language::Rust => {}
    }
    None
}

pub fn find_python_body_close(text: &str, def_offset: usize, colon_pos: usize) -> usize {
    let def_line = text[..def_offset].lines().last().unwrap_or("");
    let def_indent = def_line.len() - def_line.trim_start().len();
    let rest = &text[colon_pos + 1..];
    let mut current_offset = colon_pos + 1;
    for line in rest.lines() {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            current_offset += line.len() + 1;
            continue;
        }
        let line_indent = line.len() - line.trim_start().len();
        if line_indent <= def_indent {
            return current_offset;
        }
        current_offset += line.len() + 1;
    }
    text.len()
}

pub fn python_insertion_offset_and_indent(
    text: &str,
    colon_pos: usize,
    def_offset: usize,
) -> (usize, String) {
    let def_line = text[..def_offset].lines().last().unwrap_or("");
    let def_indent = def_line.len() - def_line.trim_start().len();
    let default_indent = " ".repeat(def_indent + 4);

    let rest = &text[colon_pos + 1..];
    let mut current_offset = colon_pos + 1;
    let mut docstring_quote = None;
    let mut in_docstring = false;

    for line in rest.lines() {
        let trimmed = line.trim();
        if !in_docstring {
            if trimmed.is_empty() || trimmed.starts_with('#') {
                current_offset += line.len() + 1;
                continue;
            }
            if let Some(after) = trimmed.strip_prefix("\"\"\"") {
                if after.contains("\"\"\"") {
                    current_offset += line.len() + 1;
                    let indent = line[..line.len() - line.trim_start().len()].to_string();
                    return (current_offset.min(text.len()), indent);
                }
                in_docstring = true;
                docstring_quote = Some("\"\"\"");
                current_offset += line.len() + 1;
                continue;
            } else if let Some(after) = trimmed.strip_prefix("'''") {
                if after.contains("'''") {
                    current_offset += line.len() + 1;
                    let indent = line[..line.len() - line.trim_start().len()].to_string();
                    return (current_offset.min(text.len()), indent);
                }
                in_docstring = true;
                docstring_quote = Some("'''");
                current_offset += line.len() + 1;
                continue;
            } else {
                let indent = line[..line.len() - line.trim_start().len()].to_string();
                return (current_offset, indent);
            }
        } else if let Some(q) = docstring_quote {
            if trimmed.contains(q) {
                current_offset += line.len() + 1;
                let indent = line[..line.len() - line.trim_start().len()].to_string();
                return (current_offset.min(text.len()), indent);
            }
            current_offset += line.len() + 1;
        }
    }
    (current_offset.min(text.len()), default_indent)
}

pub fn brace_insertion_offset_and_indent(
    text: &str,
    body_open: usize,
    body_close: usize,
    lang: Language,
) -> (usize, String) {
    let body_text = &text[body_open + 1..body_close];
    for line in body_text.lines() {
        if !line.trim().is_empty() {
            let indent = line[..line.len() - line.trim_start().len()].to_string();
            return (body_open + 1, indent);
        }
    }
    let default_indent = if lang == Language::Go {
        "\t".to_string()
    } else {
        "    ".to_string()
    };
    (body_open + 1, default_indent)
}

pub fn format_binding(
    param_name: &str,
    param_type: Option<&str>,
    val: &str,
    lang: Language,
) -> String {
    match lang {
        Language::Python => {
            format!("{param_name} = {val}")
        }
        Language::TypeScript => {
            let ty_ann = param_type.map(|t| format!(": {t}")).unwrap_or_default();
            format!("const {param_name}{ty_ann} = {val};")
        }
        Language::JavaScript => {
            format!("const {param_name} = {val};")
        }
        Language::Cpp | Language::C => {
            let mut ty = param_type.unwrap_or("auto");
            if let Some(stripped) = ty.strip_suffix(param_name) {
                let s = stripped.trim();
                if !s.is_empty() {
                    ty = s;
                }
            }
            let ty = ty.trim_start_matches("const ").trim();
            format!("const {ty} {param_name} = {val};")
        }
        Language::Swift => {
            let ty_ann = param_type.map(|t| format!(": {t}")).unwrap_or_default();
            format!("let {param_name}{ty_ann} = {val}")
        }
        Language::Go => {
            let is_const = !val.starts_with('&') && !val.contains('{') && val != "nil";
            let kw = if is_const { "const" } else { "var" };
            format!("{kw} {param_name} = {val}")
        }
        Language::Rust => {
            let ty_ann = param_type.map(|t| format!(": {t}")).unwrap_or_default();
            format!("let {param_name}{ty_ann} = {val};")
        }
        Language::Java => {
            let ty = param_type.unwrap_or("var");
            format!("{ty} {param_name} = {val};")
        }
    }
}

pub fn find_polyglot_declaration(
    text: &str,
    lang: Language,
    line: Option<u32>,
    function: Option<&str>,
) -> Result<PolyglotDecl> {
    let clean_name = function
        .map(|f| {
            f.rsplit_once("::")
                .map(|(_, m)| m)
                .or_else(|| f.rsplit_once('.').map(|(_, m)| m))
                .unwrap_or(f)
                .trim()
                .to_string()
        })
        .or_else(|| {
            let l = line?;
            let lines: Vec<&str> = text.lines().collect();
            if l == 0 || l as usize > lines.len() {
                return None;
            }
            let target_idx = (l - 1) as usize;
            let start_idx = target_idx.saturating_sub(3);
            let end_idx = (target_idx + 3).min(lines.len().saturating_sub(1));
            for i in (start_idx..=end_idx).rev() {
                if let Some(name) = extract_decl_name_from_line(lines[i], lang) {
                    return Some(name);
                }
            }
            None
        })
        .context("could not determine function name to inline parameter from")?;

    // Search for declaration of `clean_name`
    let needle_paren = format!("{clean_name}(");
    let needle_space_paren = format!("{clean_name} (");
    let needle_generic = format!("{clean_name}<");

    let mut found_decl = None;
    for (pos, _) in text
        .match_indices(&needle_paren)
        .chain(text.match_indices(&needle_space_paren))
        .chain(text.match_indices(&needle_generic))
    {
        // Preceding char check
        if pos > 0 {
            let prev = text[..pos].chars().next_back().unwrap();
            if is_ident(prev) {
                continue;
            }
        }
        let after_name = pos + clean_name.len();
        let open_paren = match text[after_name..].find('(') {
            Some(p) => after_name + p,
            None => continue,
        };
        let close_paren = match crate::parameter_object::matching_bracket(text, open_paren) {
            Some(p) => p,
            None => continue,
        };

        let (body_open, body_close) = if lang == Language::Python {
            let colon = match text[close_paren..].find(':') {
                Some(c) => close_paren + c,
                None => continue,
            };
            let b_close = find_python_body_close(text, pos, colon);
            (colon, b_close)
        } else {
            let b_open = match text[close_paren..].find('{') {
                Some(b) => close_paren + b,
                None => continue,
            };
            let b_close = match crate::parameter_object::matching_bracket(text, b_open) {
                Some(b) => b,
                None => continue,
            };
            (b_open, b_close)
        };

        let (receiver, params) =
            crate::parameter_object::parse_params(&text[open_paren + 1..close_paren], lang);
        found_decl = Some(PolyglotDecl {
            fn_name: clean_name.clone(),
            open_paren,
            close_paren,
            body_open,
            body_close,
            receiver,
            params,
        });
        break;
    }

    found_decl.with_context(|| format!("could not find declaration for function `{clean_name}`"))
}
