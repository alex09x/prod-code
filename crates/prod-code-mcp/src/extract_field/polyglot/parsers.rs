/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;

use crate::extract_field::helpers::is_ident;
use crate::parameter_object::Language;

pub(crate) fn language_matches_extension(lang: Language, path: &Path) -> bool {
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
    match lang {
        Language::Rust => ext == "rs",
        Language::TypeScript => matches!(ext, "ts" | "tsx"),
        Language::JavaScript => matches!(ext, "js" | "jsx" | "mjs" | "cjs"),
        Language::Python => ext == "py",
        Language::Go => ext == "go",
        Language::Swift => ext == "swift",
        Language::Cpp => matches!(ext, "cpp" | "cc" | "cxx" | "hpp" | "h"),
        Language::C => matches!(ext, "c" | "h"),
        Language::Java => ext == "java",
    }
}

pub(crate) fn parse_params_python(params_str: &str) -> Vec<String> {
    let mut out = Vec::new();
    for part in params_str.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() || trimmed == "*" || trimmed == "/" {
            continue;
        }
        let name = trimmed.split([':', '=']).next().unwrap_or("").trim();
        let name = name.strip_prefix('*').unwrap_or(name).trim();
        let name = name.strip_prefix('*').unwrap_or(name).trim();
        if !name.is_empty() && name.chars().all(is_ident) && name != "self" {
            out.push(name.to_string());
        }
    }
    out
}

pub(crate) fn parse_params_ts(params_str: &str) -> Vec<String> {
    let mut out = Vec::new();
    for part in params_str.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        let head = trimmed.split([':', '=']).next().unwrap_or("").trim();
        let name = head
            .trim_start_matches("public ")
            .trim_start_matches("private ")
            .trim_start_matches("protected ")
            .trim_start_matches("readonly ")
            .trim();
        if !name.is_empty() && name.chars().all(is_ident) && name != "this" {
            out.push(name.to_string());
        }
    }
    out
}

pub(crate) fn parse_params_go(params_str: &str) -> Vec<String> {
    let mut out = Vec::new();
    for part in params_str.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        let name = trimmed.split_whitespace().next().unwrap_or("").trim();
        if !name.is_empty() && name.chars().all(is_ident) {
            out.push(name.to_string());
        }
    }
    out
}

pub(crate) fn parse_params_swift(params_str: &str) -> Vec<String> {
    let mut out = Vec::new();
    for part in params_str.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        let head = trimmed.split(':').next().unwrap_or("").trim();
        let words: Vec<&str> = head.split_whitespace().collect();
        let name = words.last().copied().unwrap_or("");
        if !name.is_empty() && name.chars().all(is_ident) && name != "_" && name != "self" {
            out.push(name.to_string());
        }
    }
    out
}

pub(crate) fn parse_params_cpp(params_str: &str) -> Vec<String> {
    let mut out = Vec::new();
    for part in params_str.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        let before_default = trimmed.split('=').next().unwrap_or("").trim();
        let name = before_default
            .split(['*', '&', ' '])
            .rfind(|s| !s.is_empty())
            .unwrap_or("");
        if !name.is_empty() && name.chars().all(is_ident) && name != "this" {
            out.push(name.to_string());
        }
    }
    out
}

pub(crate) fn parse_locals(text: &str, lang: Language) -> Vec<String> {
    let mut locals = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        match lang {
            Language::TypeScript | Language::JavaScript => {
                for prefix in ["const ", "let ", "var "] {
                    if let Some(after) = trimmed.strip_prefix(prefix) {
                        let name = after
                            .split([':', '=', ' ', ';'])
                            .next()
                            .unwrap_or("")
                            .trim();
                        if !name.is_empty() && name.chars().all(is_ident) {
                            locals.push(name.to_string());
                        }
                    }
                }
            }
            Language::Python => {
                if !trimmed.starts_with('#')
                    && trimmed.contains('=')
                    && !trimmed.contains("==")
                    && !trimmed.starts_with("self.")
                {
                    let name = trimmed.split(['=', ':']).next().unwrap_or("").trim();
                    if !name.is_empty()
                        && name.chars().all(is_ident)
                        && name != "def"
                        && name != "class"
                    {
                        locals.push(name.to_string());
                    }
                }
            }
            Language::Go => {
                if let Some((before, _)) = trimmed.split_once(":=") {
                    let name = before.trim();
                    if !name.is_empty() && name.chars().all(is_ident) {
                        locals.push(name.to_string());
                    }
                }
            }
            Language::Swift => {
                for prefix in ["let ", "var "] {
                    if let Some(after) = trimmed.strip_prefix(prefix) {
                        let name = after.split([':', '=', ' ']).next().unwrap_or("").trim();
                        if !name.is_empty() && name.chars().all(is_ident) {
                            locals.push(name.to_string());
                        }
                    }
                }
            }
            Language::Cpp | Language::C => {
                if let Some((before, _)) = trimmed.split_once('=') {
                    let words: Vec<&str> = before.split_whitespace().collect();
                    if words.len() >= 2 {
                        let name = words
                            .last()
                            .copied()
                            .unwrap_or("")
                            .trim_start_matches('*')
                            .trim_start_matches('&');
                        if !name.is_empty() && name.chars().all(is_ident) {
                            locals.push(name.to_string());
                        }
                    }
                }
            }
            _ => {}
        }
    }
    locals
}

pub(crate) fn has_member_named(text: &str, lang: Language, owner: &str, name: &str) -> bool {
    match lang {
        Language::Go => {
            let needle = format!("type {owner} struct");
            let Some(struct_at) = text.find(&needle) else {
                return false;
            };
            let Some(s_open) = text[struct_at..].find('{').map(|i| struct_at + i) else {
                return false;
            };
            let Some(s_close) = crate::parameter_object::matching_bracket(text, s_open) else {
                return false;
            };
            text[s_open + 1..s_close]
                .lines()
                .any(|l| l.split_whitespace().next() == Some(name))
        }
        Language::Python => {
            let needle = format!("class {owner}");
            let Some(c_at) = text.find(&needle) else {
                return false;
            };
            let lines: Vec<&str> = text[c_at..].lines().collect();
            if lines.is_empty() {
                return false;
            }
            let c_indent = lines[0].len() - lines[0].trim_start().len();
            for line in lines.into_iter().skip(1) {
                let trimmed = line.trim_start();
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    continue;
                }
                let ind = line.len() - trimmed.len();
                if ind <= c_indent {
                    break;
                }
                let t = trimmed;
                if t.starts_with(&format!("self.{name} ="))
                    || t.starts_with(&format!("self.{name}:"))
                    || t.starts_with(&format!("{name} ="))
                    || t.starts_with(&format!("{name}:"))
                    || t.starts_with(&format!("def {name}("))
                {
                    return true;
                }
            }
            false
        }
        Language::TypeScript | Language::JavaScript => {
            let needle = format!("class {owner}");
            let Some(c_at) = text.find(&needle) else {
                return false;
            };
            let Some(c_open) = text[c_at..].find('{').map(|i| c_at + i) else {
                return false;
            };
            let Some(c_close) = crate::parameter_object::matching_bracket(text, c_open) else {
                return false;
            };
            text[c_open + 1..c_close].lines().any(|l| {
                let t = l
                    .trim()
                    .trim_start_matches("public ")
                    .trim_start_matches("private ")
                    .trim_start_matches("protected ")
                    .trim_start_matches("readonly ")
                    .trim_start_matches("static ")
                    .trim();
                t.starts_with(&format!("{name}:"))
                    || t.starts_with(&format!("{name} ="))
                    || t.starts_with(&format!("{name}("))
                    || t.starts_with(&format!("{name}?:"))
            })
        }
        Language::Swift => {
            let mut c_open_opt = None;
            for kind in ["class ", "struct ", "actor "] {
                let needle = format!("{kind}{owner}");
                if let Some(c_at) = text.find(&needle)
                    && let Some(c_open) = text[c_at..].find('{').map(|i| c_at + i)
                {
                    c_open_opt = Some(c_open);
                    break;
                }
            }
            let Some(c_open) = c_open_opt else {
                return false;
            };
            let Some(c_close) = crate::parameter_object::matching_bracket(text, c_open) else {
                return false;
            };
            text[c_open + 1..c_close].lines().any(|l| {
                let t = l.trim();
                t.starts_with(&format!("var {name}"))
                    || t.starts_with(&format!("let {name}"))
                    || t.starts_with(&format!("func {name}("))
            })
        }
        Language::Cpp | Language::C => {
            let mut c_open_opt = None;
            for kind in ["class ", "struct "] {
                let needle = format!("{kind}{owner}");
                if let Some(c_at) = text.find(&needle)
                    && let Some(c_open) = text[c_at..].find('{').map(|i| c_at + i)
                {
                    c_open_opt = Some(c_open);
                    break;
                }
            }
            let Some(c_open) = c_open_opt else {
                return false;
            };
            let Some(c_close) = crate::parameter_object::matching_bracket(text, c_open) else {
                return false;
            };
            text[c_open + 1..c_close].lines().any(|l| {
                let t = l.trim();
                t.contains(&format!(" {name};"))
                    || t.contains(&format!(" {name} ="))
                    || t.contains(&format!(" {name}("))
                    || t.starts_with(&format!("{name};"))
                    || t.starts_with(&format!("{name} ="))
                    || t.starts_with(&format!("{name}("))
            })
        }
        Language::Rust | Language::Java => false,
    }
}
