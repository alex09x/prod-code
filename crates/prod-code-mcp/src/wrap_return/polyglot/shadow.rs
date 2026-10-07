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

/// Checks if `name` at byte offset `at` is shadowed by an enclosing local variable or parameter.
/// Used during empty-reference fallback to prevent rewriting calls to local parameters/bindings (#985).
pub(crate) fn is_locally_shadowed(content: &str, at: usize, name: &str, lang: Language) -> bool {
    if lang == Language::Python {
        return is_python_shadowed(content, at, name);
    }

    // Check if `at` itself is inside a parameter list of a function header:
    if let Some(open_p) = content[..at].rfind('(') {
        if let Some(close_p) = crate::parameter_object::matching_bracket(content, open_p) {
            if open_p < at && at < close_p {
                let after_paren = content[close_p + 1..].trim_start();
                if let Some(open_b) = after_paren.find('{') {
                    if open_b < 200 && !after_paren[..open_b].contains(';') {
                        let params = &content[open_p + 1..close_p];
                        if params_declare_name(params, name, lang) {
                            return true;
                        }
                    }
                }
            }
        }
    }

    // Check if `at` itself is a local variable declaration (const name =, let name =, etc.)
    let line_start = content[..at].rfind('\n').map_or(0, |p| p + 1);
    let line_before = content[line_start..at].trim();
    if let Some(rest) = line_before
        .strip_prefix("const ")
        .or_else(|| line_before.strip_prefix("let "))
        .or_else(|| line_before.strip_prefix("var "))
        .or_else(|| line_before.strip_prefix("mut "))
    {
        if !rest.contains('=') && (rest.trim().is_empty() || rest.starts_with('{')) {
            return true;
        }
    } else if content[at + name.len()..].trim_start().starts_with(":=") {
        return true;
    }

    // Bracket-scoped languages (TS, JS, Go, Rust, C, C++, Swift)
    let mut search = at;
    while let Some(open_brace) = content[..search].rfind('{') {
        search = open_brace;
        let Some(close_brace) = crate::parameter_object::matching_bracket(content, open_brace)
        else {
            continue;
        };
        if open_brace < at && at < close_brace {
            // 1. Check local variable declarations between open_brace and at
            let body_prefix = &content[open_brace + 1..at];
            if body_has_local_decl(body_prefix, name, lang) {
                return true;
            }

            // 2. Check parameter list before open_brace
            if let Some(close_paren) = content[..open_brace].rfind(')') {
                let between = &content[close_paren + 1..open_brace];
                if between.len() < 200 {
                    if let Some(open_paren) = find_matching_open_paren(content, close_paren) {
                        let params = &content[open_paren + 1..close_paren];
                        if params_declare_name(params, name, lang) {
                            return true;
                        }
                    }
                }
            }
        }
    }
    false
}

fn find_matching_open_paren(text: &str, close_paren: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (idx, c) in text[..=close_paren].char_indices().rev() {
        if c == ')' {
            depth += 1;
        } else if c == '(' {
            depth -= 1;
            if depth == 0 {
                return Some(idx);
            }
        }
    }
    None
}

fn params_declare_name(params: &str, name: &str, lang: Language) -> bool {
    for chunk in params.split(',') {
        let chunk = chunk.trim();
        if chunk.is_empty() {
            continue;
        }
        match lang {
            Language::TypeScript | Language::JavaScript | Language::Swift | Language::Rust => {
                let ident = chunk.split(':').next().unwrap_or(chunk);
                let ident = ident.split('=').next().unwrap_or(ident).trim();
                let ident = ident.trim_start_matches("mut ").trim();
                if ident == name {
                    return true;
                }
                // Destructured param: { retry } or { orig: retry }
                if let (Some(o), Some(c)) = (ident.find('{'), ident.rfind('}')) {
                    if o < c {
                        for field in ident[o + 1..c].split(',') {
                            let field = field.trim();
                            if let Some((_, local)) = field.split_once(':') {
                                if local.trim() == name {
                                    return true;
                                }
                            } else if field == name {
                                return true;
                            }
                        }
                    }
                }
            }
            Language::Go => {
                let words: Vec<&str> = chunk.split_whitespace().collect();
                if words.first() == Some(&name) {
                    return true;
                }
            }
            _ => {
                let words: Vec<&str> = chunk.split_whitespace().collect();
                if words.contains(&name) {
                    return true;
                }
            }
        }
    }
    false
}

fn body_has_local_decl(body_prefix: &str, name: &str, lang: Language) -> bool {
    for line in body_prefix.lines() {
        let trimmed = line.trim();
        match lang {
            Language::TypeScript | Language::JavaScript => {
                if let Some(rest) = trimmed
                    .strip_prefix("const ")
                    .or_else(|| trimmed.strip_prefix("let "))
                    .or_else(|| trimmed.strip_prefix("var "))
                {
                    if params_declare_name(rest, name, lang) {
                        return true;
                    }
                }
            }
            Language::Rust => {
                if let Some(rest) = trimmed.strip_prefix("let ") {
                    let rest = rest.trim_start_matches("mut ").trim();
                    if params_declare_name(rest, name, lang) {
                        return true;
                    }
                }
            }
            Language::Go => {
                if let Some((lhs, _)) = trimmed.split_once(":=") {
                    for v in lhs.split(',') {
                        if v.trim() == name {
                            return true;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    false
}

fn is_python_shadowed(content: &str, at: usize, name: &str) -> bool {
    let lines: Vec<&str> = content[..at].lines().collect();
    let target_line = match lines.last() {
        Some(l) => *l,
        None => return false,
    };
    let target_indent = target_line.len() - target_line.trim_start().len();
    for (i, line) in lines.iter().enumerate().rev().skip(1) {
        let trimmed = line.trim();
        if trimmed.starts_with("def ") || trimmed.starts_with("async def ") {
            let indent = line.len() - line.trim_start().len();
            if indent < target_indent {
                if let (Some(open), Some(close)) = (line.find('('), line.rfind(')')) {
                    let params = &line[open + 1..close];
                    for p in params.split(',') {
                        let p = p
                            .trim()
                            .split(':')
                            .next()
                            .unwrap()
                            .split('=')
                            .next()
                            .unwrap()
                            .trim();
                        if p == name {
                            return true;
                        }
                    }
                }
                for body_line in &lines[i + 1..lines.len() - 1] {
                    let b_trimmed = body_line.trim();
                    if let Some((lhs, _)) = b_trimmed.split_once('=') {
                        let var = lhs.trim().split(':').next().unwrap().trim();
                        if var == name {
                            return true;
                        }
                    }
                }
                break;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_locally_shadowed_by_param() {
        let content = "function run(retry: () => void) {\n    retry();\n}\n";
        let call_at = content.find("retry();").unwrap();
        assert!(is_locally_shadowed(
            content,
            call_at,
            "retry",
            Language::TypeScript
        ));

        let content_no_shadow = "function run() {\n    retry();\n}\n";
        let call_at = content_no_shadow.find("retry();").unwrap();
        assert!(!is_locally_shadowed(
            content_no_shadow,
            call_at,
            "retry",
            Language::TypeScript
        ));
    }

    #[test]
    fn test_is_locally_shadowed_by_local_var() {
        let content = "function run() {\n    const retry = () => {};\n    retry();\n}\n";
        let call_at = content.find("retry();").unwrap();
        assert!(is_locally_shadowed(
            content,
            call_at,
            "retry",
            Language::TypeScript
        ));
    }

    #[test]
    fn test_is_python_shadowed() {
        let content = "def run(retry):\n    retry()\n";
        let call_at = content.find("retry()").unwrap();
        assert!(is_locally_shadowed(
            content,
            call_at,
            "retry",
            Language::Python
        ));

        let content_no_shadow = "def run():\n    retry()\n";
        let call_at = content_no_shadow.find("retry()").unwrap();
        assert!(!is_locally_shadowed(
            content_no_shadow,
            call_at,
            "retry",
            Language::Python
        ));
    }
}
