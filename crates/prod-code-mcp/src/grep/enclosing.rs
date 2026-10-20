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

/// Finds the enclosing function, method, or type declaration at `target_line_1based` in `path`.
pub fn find_enclosing_symbol_in_file(path: &Path, target_line_1based: u32) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let lines: Vec<&str> = content.lines().collect();
    find_enclosing_symbol_in_lines(&lines, target_line_1based)
}

/// Finds the enclosing function, method, or type declaration at `target_line_1based` in `lines`.
/// Returns a qualified name (e.g. `Type::method` or `Type.method`) when inside a container, so
/// agents can immediately pass the symbol to `code_definition` or `code_references`.
pub fn find_enclosing_symbol_in_lines<S: AsRef<str>>(
    lines: &[S],
    target_line_1based: u32,
) -> Option<String> {
    if lines.is_empty() || target_line_1based == 0 {
        return None;
    }
    let refs: Vec<&str> = lines.iter().map(AsRef::as_ref).collect();
    find_enclosing_symbol_in_str_lines(&refs, target_line_1based)
}

fn find_enclosing_symbol_in_str_lines(lines: &[&str], target_line_1based: u32) -> Option<String> {
    let idx = (target_line_1based as usize)
        .saturating_sub(1)
        .min(lines.len() - 1);

    // Check if the file looks like Python (indentation-scoped `def` / `class`).
    if is_python_like(lines, idx) {
        return find_enclosing_python_symbol(lines, idx);
    }

    // Brace-scoped scan from the start of the file up to `idx`.
    let mut stack: Vec<(usize, ScopeKind, String)> = Vec::new();
    let mut brace_depth: usize = 0;
    let mut pending: Option<(ScopeKind, String)> = None;

    for (line_idx, raw_line) in lines.iter().enumerate().take(idx + 1) {
        let trimmed = strip_line_comment(raw_line).trim();
        let in_container = stack
            .iter()
            .any(|(_, k, _)| matches!(k, ScopeKind::Container));
        if !trimmed.is_empty()
            && let Some(decl) = parse_declaration_header(trimmed, in_container)
        {
            if line_idx == idx {
                let container = stack
                    .iter()
                    .rev()
                    .find_map(|(_, k, n)| matches!(k, ScopeKind::Container).then(|| n.as_str()));
                return Some(qualify_symbol(container, &decl.1));
            }
            pending = Some(decl);
        }

        for ch in trimmed.chars() {
            if ch == '{' {
                brace_depth += 1;
                if let Some((kind, name)) = pending.take() {
                    stack.push((brace_depth, kind, name));
                }
            } else if ch == '}' {
                if line_idx < idx {
                    while stack.last().is_some_and(|(d, _, _)| *d >= brace_depth) {
                        stack.pop();
                    }
                }
                brace_depth = brace_depth.saturating_sub(1);
            }
        }
        if trimmed.ends_with(';') {
            pending = None;
        }
    }

    let func = stack
        .iter()
        .rev()
        .find_map(|(_, k, n)| matches!(k, ScopeKind::Callable).then(|| n.as_str()));
    let container = stack
        .iter()
        .rev()
        .find_map(|(_, k, n)| matches!(k, ScopeKind::Container).then(|| n.as_str()));

    match (container, func) {
        (Some(c), Some(f)) => Some(qualify_symbol(Some(c), f)),
        (None, Some(f)) => Some(f.to_string()),
        (Some(c), None) => Some(c.to_string()),
        (None, None) => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScopeKind {
    Container,
    Callable,
}

fn qualify_symbol(container: Option<&str>, member: &str) -> String {
    match container {
        Some(c) if !c.is_empty() && !member.contains("::") && !member.contains('.') => {
            format!("{c}::{member}")
        }
        _ => member.to_string(),
    }
}

fn is_python_like(lines: &[&str], max_idx: usize) -> bool {
    lines.iter().take(max_idx + 1).any(|l| {
        let t = l.trim_start();
        (t.starts_with("def ") || t.starts_with("async def ") || t.starts_with("class "))
            && t.trim_end().ends_with(':')
    })
}

fn find_enclosing_python_symbol(lines: &[&str], idx: usize) -> Option<String> {
    let mut current_indent = lines[idx].chars().take_while(|c| c.is_whitespace()).count();
    let trimmed_target = lines[idx].trim();
    if let Some((_, name)) = parse_python_header(trimmed_target) {
        // If target line itself is a def/class, look for an enclosing class with smaller indent.
        for prev in (0..idx).rev() {
            let line = lines[prev];
            let t = line.trim();
            if t.is_empty() || t.starts_with('#') {
                continue;
            }
            let indent = line.chars().take_while(|c| c.is_whitespace()).count();
            if indent < current_indent
                && let Some((ScopeKind::Container, cls)) = parse_python_header(t)
            {
                return Some(format!("{cls}.{name}"));
            }
        }
        return Some(name);
    }

    // If target line is blank or comment, allow any indent above it.
    if trimmed_target.is_empty() || trimmed_target.starts_with('#') {
        current_indent = usize::MAX;
    }

    let mut found_func: Option<(usize, String)> = None;
    let mut found_class: Option<String> = None;

    for prev in (0..=idx).rev() {
        let line = lines[prev];
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let indent = line.chars().take_while(|c| c.is_whitespace()).count();
        if indent < current_indent
            && let Some((kind, name)) = parse_python_header(t)
        {
            match kind {
                ScopeKind::Callable if found_func.is_none() => {
                    found_func = Some((indent, name));
                    current_indent = indent;
                }
                ScopeKind::Container => {
                    found_class = Some(name);
                    break;
                }
                _ => {}
            }
        }
    }

    match (found_class, found_func) {
        (Some(c), Some((_, f))) => Some(format!("{c}.{f}")),
        (None, Some((_, f))) => Some(f),
        (Some(c), None) => Some(c),
        (None, None) => None,
    }
}

fn parse_python_header(trimmed: &str) -> Option<(ScopeKind, String)> {
    let rest = trimmed
        .strip_prefix("async def ")
        .or_else(|| trimmed.strip_prefix("def "));
    if let Some(r) = rest {
        let name = take_ident(r)?;
        return Some((ScopeKind::Callable, name));
    }
    if let Some(r) = trimmed.strip_prefix("class ") {
        let name = take_ident(r)?;
        return Some((ScopeKind::Container, name));
    }
    None
}

fn parse_declaration_header(trimmed: &str, in_container: bool) -> Option<(ScopeKind, String)> {
    if trimmed.starts_with("//") || trimmed.starts_with('#') || trimmed.starts_with("/*") {
        return None;
    }
    // Rust `impl` blocks: `impl Foo`, `impl<T> Trait for Foo<T>`
    if let Some(after_impl) = strip_keyword_after_modifiers(trimmed, "impl") {
        let target = extract_impl_target(after_impl)?;
        return Some((ScopeKind::Container, target));
    }
    // Go function or method: `func (s *Store) Method(...)` or `func Name(...)`
    if let Some(after_func) = trimmed.strip_prefix("func ") {
        let after_func = after_func.trim_start();
        if let Some(in_recv) = after_func.strip_prefix('(')
            && let Some((recv, rest)) = in_recv.split_once(')')
        {
            let recv_type = recv
                .split_whitespace()
                .last()
                .map(|t| t.trim_start_matches('*'))
                .and_then(take_ident);
            let method = take_ident(rest.trim_start())?;
            let full = match recv_type {
                Some(rt) => format!("{rt}.{method}"),
                None => method,
            };
            return Some((ScopeKind::Callable, full));
        }
        if let Some(name) = take_ident(after_func) {
            return Some((ScopeKind::Callable, name));
        }
    }
    // Go `type Name struct` / `type Name interface`
    if let Some(after_type) = trimmed.strip_prefix("type ")
        && let Some(name) = take_ident(after_type.trim_start())
        && (after_type.contains(" struct") || after_type.contains(" interface"))
    {
        return Some((ScopeKind::Container, name));
    }
    // Rust / Swift / TS / Kotlin functions: `fn name`, `func name`, `function name`, `fun name`
    for kw in ["fn ", "func ", "function ", "fun "] {
        if let Some(rest) = strip_keyword_after_modifiers(trimmed, kw.trim())
            && let Some(name) = take_ident(rest)
        {
            return Some((ScopeKind::Callable, name));
        }
    }
    // Containers: `struct`, `enum`, `trait`, `class`, `interface`, `protocol`, `actor`, `extension`
    for kw in [
        "struct",
        "enum",
        "trait",
        "class",
        "interface",
        "protocol",
        "actor",
        "extension",
    ] {
        if let Some(rest) = strip_keyword_after_modifiers(trimmed, kw)
            && let Some(name) = take_ident(rest)
        {
            return Some((ScopeKind::Container, name));
        }
    }
    // TS / JS / Java / C# class methods: `[modifiers...] methodName(...) {`
    if in_container && trimmed.ends_with('{') {
        let mut s = trimmed;
        while let Some(next) = s.find(char::is_whitespace) {
            let tok = &s[..next];
            if is_modifier_token(tok) {
                s = s[next..].trim_start();
            } else {
                break;
            }
        }
        if let Some(name) = take_ident(s)
            && !matches!(
                name.as_str(),
                "if" | "for" | "while" | "switch" | "catch" | "return" | "super" | "new"
            )
            && s[name.len()..].trim_start().starts_with(['(', '<'])
        {
            return Some((ScopeKind::Callable, name));
        }
    }
    None
}

fn strip_keyword_after_modifiers<'a>(line: &'a str, kw: &str) -> Option<&'a str> {
    let mut s = line.trim_start();
    loop {
        if let Some(rest) = s.strip_prefix(kw) {
            if kw == "impl" && rest.starts_with('<') {
                return Some(skip_angle_brackets(rest));
            }
            if rest.starts_with(char::is_whitespace) {
                return Some(rest.trim_start());
            }
        }
        let next = s.find(char::is_whitespace)?;
        let token = &s[..next];
        if !is_modifier_token(token) {
            return None;
        }
        s = s[next..].trim_start();
    }
}

fn is_modifier_token(tok: &str) -> bool {
    matches!(
        tok,
        "pub"
            | "async"
            | "unsafe"
            | "const"
            | "extern"
            | "export"
            | "default"
            | "public"
            | "private"
            | "internal"
            | "fileprivate"
            | "open"
            | "final"
            | "static"
            | "override"
            | "mutating"
            | "nonisolated"
            | "abstract"
            | "suspend"
    ) || tok.starts_with("pub(")
        || tok.starts_with('@')
        || tok.starts_with('"')
}

fn extract_impl_target(after_impl: &str) -> Option<String> {
    let s = skip_angle_brackets(after_impl.trim_start()).trim_start();
    let head = s.split(['{', '(']).next()?.split("where").next()?.trim();
    let target_part = if let Some((_, rhs)) = head.rsplit_once(" for ") {
        rhs.trim()
    } else {
        head
    };
    let clean = target_part.trim_start_matches(['&', '*']).trim_start();
    take_ident(clean)
}

fn skip_angle_brackets(s: &str) -> &str {
    let s = s.trim_start();
    if !s.starts_with('<') {
        return s;
    }
    let mut depth = 0usize;
    for (idx, ch) in s.char_indices() {
        match ch {
            '<' => depth += 1,
            '>' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return &s[idx + 1..];
                }
            }
            _ => {}
        }
    }
    s
}

fn take_ident(s: &str) -> Option<String> {
    let ident: String = s
        .trim_start()
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    (!ident.is_empty() && !ident.chars().next().unwrap().is_ascii_digit()).then_some(ident)
}

fn strip_line_comment(line: &str) -> &str {
    if let Some((before, _)) = line.split_once("//") {
        before
    } else {
        line
    }
}
