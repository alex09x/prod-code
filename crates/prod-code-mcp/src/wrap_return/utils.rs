/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::Wrapper;
use crate::parameter_object::Language;
use std::path::Path;

pub(crate) fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

pub(crate) fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

pub(crate) fn one_based_lsp_position(text: &str, byte_offset: usize) -> (u32, u32) {
    let before = &text[..byte_offset];
    let line = before.bytes().filter(|byte| *byte == b'\n').count() as u32 + 1;
    let column_text = before.rsplit('\n').next().unwrap_or_default();
    let character = column_text.encode_utf16().count() as u32 + 1;
    (line, character)
}

pub(crate) fn is_import_export_call_context(text: &str, at: usize, lang: Language) -> bool {
    if !matches!(lang, Language::TypeScript | Language::JavaScript) {
        return crate::inline_parameter::is_import_or_export_context(text, at, lang);
    }
    let line_start = text[..at].rfind('\n').map_or(0, |position| position + 1);
    let line_end = text[at..]
        .find('\n')
        .map_or(text.len(), |offset| at + offset);
    let line = text[line_start..line_end].trim_start();
    if line.starts_with("import ")
        || line.starts_with("import{")
        || line.starts_with("export {")
        || line.starts_with("export{")
        || line.starts_with("export *")
        || line.starts_with("from ")
        || has_require_call(line)
    {
        return true;
    }
    let before = &text[..at];
    before
        .rfind("import {")
        .or_else(|| before.rfind("export {"))
        .is_some_and(|start| !before[start..].contains('}'))
}

pub(crate) fn has_require_call(s: &str) -> bool {
    let mut search = s;
    while let Some(pos) = search.find("require") {
        let before_ok = pos == 0 || {
            let prev = search[..pos].chars().next_back().unwrap();
            !prev.is_alphanumeric() && prev != '_' && prev != '$'
        };
        let after = &search[pos + "require".len()..];
        let not_ident = after
            .chars()
            .next()
            .map_or(true, |c| !c.is_alphanumeric() && c != '_' && c != '$');
        let trimmed = after.trim_start();
        if before_ok && not_ident && trimmed.starts_with('(') {
            return true;
        }
        search = &search[pos + "require".len()..];
    }
    false
}

/// The return type a function header declares between its parameter list's `)` at `close` and
/// its body's `{`, or `None` for a function that returns `()` implicitly.
pub fn declared_return(text: &str, close: usize) -> Option<(usize, usize)> {
    let body = text[close..].find(['{', ';']).map(|i| close + i)?;
    let header = &text[close + 1..body];
    let arrow = header.find("->")?;
    let start = close + 1 + arrow + 2;
    let mut end = body;
    if let Some(w) = text[start..body].find(" where") {
        end = start + w;
    }
    let lead = text[start..end].len() - text[start..end].trim_start().len();
    let trail = text[start..end].len() - text[start..end].trim_end().len();
    Some((start + lead, end - trail))
}

pub(crate) fn cpp_return_type_span(
    text: &str,
    decl_start: usize,
    name_start: usize,
) -> (String, Option<(usize, usize)>) {
    let prefix = &text[decl_start..name_start];
    let leading = prefix.len() - prefix.trim_start().len();
    let mut start = decl_start + leading;
    let mut end = decl_start + prefix.trim_end().len();

    // In an out-of-class definition, remove the trailing class scope qualifier from the
    // return-type region while preserving it in the declaration.
    if text[start..end].ends_with("::") {
        let before_scope = &text[start..end - 2];
        let Some(split) = before_scope.rfind(char::is_whitespace) else {
            return (String::new(), None);
        };
        end = start + split;
        while end > start
            && text[end - 1..end]
                .chars()
                .next()
                .is_some_and(char::is_whitespace)
        {
            end -= 1;
        }
    }

    const SPECIFIERS: &[&str] = &[
        "static",
        "inline",
        "virtual",
        "constexpr",
        "consteval",
        "friend",
        "extern",
        "explicit",
        "register",
    ];
    loop {
        let remaining = &text[start..end];
        let token_end = remaining
            .find(char::is_whitespace)
            .unwrap_or(remaining.len());
        let token = &remaining[..token_end];
        if !SPECIFIERS.contains(&token) {
            break;
        }
        start += token_end;
        while start < end {
            let ch = text[start..].chars().next().unwrap();
            if !ch.is_whitespace() {
                break;
            }
            start += ch.len_utf8();
        }
    }
    let type_region = &text[start..end];
    let type_start = start + type_region.len() - type_region.trim_start().len();
    let type_end = end - (type_region.len() - type_region.trim_end().len());
    if type_start >= type_end {
        return (String::new(), None);
    }
    (
        text[type_start..type_end].to_string(),
        Some((type_start, type_end)),
    )
}

/// The innermost function whose body contains `at`, and the return type its header declares
/// (`()` when it declares none).
pub fn enclosing_return_type(text: &str, at: usize) -> Option<String> {
    let mut search = at;
    while let Some(fn_at) = text[..search].rfind("fn ") {
        search = fn_at;
        if text[..fn_at]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
        {
            continue;
        }
        let Some((_, _, close)) = crate::signature::param_span(text, fn_at + 3) else {
            continue;
        };
        let Some(open) = text[close..].find(['{', ';']).map(|i| close + i) else {
            continue;
        };
        if text.as_bytes()[open] != b'{' {
            continue;
        }
        let Some(end) = crate::parameter_object::matching_bracket(text, open) else {
            continue;
        };
        if open < at && at < end {
            return Some(
                declared_return(text, close)
                    .map(|(s, e)| text[s..e].to_string())
                    .unwrap_or_else(|| "()".to_string()),
            );
        }
    }
    None
}

/// Whether a function returning `ty` can apply `?` (or propagate) a value wrapped in `wrapper`.
pub fn propagates(ty: &str, wrapper: &Wrapper) -> bool {
    let head = ty.trim().split('<').next().unwrap_or("").trim();
    let head = head.split('[').next().unwrap_or(head).trim();
    let last = head.rsplit("::").next().unwrap_or(head);
    let last = last.rsplit('.').next().unwrap_or(last).trim();
    match wrapper {
        Wrapper::Option => {
            last == "Option"
                || last == "Optional"
                || ty.trim().ends_with('?')
                || ty.trim().contains("| null")
                || ty.trim().contains("| None")
                || ty.trim().starts_with('*')
        }
        Wrapper::Result => last == "Result" || last == "expected" || last == "error",
        Wrapper::Promise => last == "Promise" || last == "Future",
        Wrapper::Pointer => ty.trim().starts_with('*'),
        Wrapper::Custom(custom_name) => {
            let custom_head = custom_name
                .trim()
                .split('<')
                .next()
                .unwrap_or(custom_name)
                .trim();
            let custom_head = custom_head.split('[').next().unwrap_or(custom_head).trim();
            let custom_last = custom_head.rsplit("::").next().unwrap_or(custom_head);
            let custom_last = custom_last.rsplit('.').next().unwrap_or(custom_last).trim();
            last == custom_last || ty.contains(custom_last)
        }
    }
}

/// Formats a constructor or factory call for a wrapped return expression.
pub(crate) fn format_constructor_call(
    constructor: Option<&str>,
    default_base: &str,
    expr: &str,
    lang: Language,
    was: &str,
) -> String {
    let expr = expr.trim();
    if let Some(ctor) = constructor {
        let ctor = ctor.trim();
        if ctor.contains("{expr}") {
            return ctor.replace("{expr}", expr);
        }
        if ctor.contains("{}") {
            return ctor.replace("{}", expr);
        }
        if expr.is_empty() {
            return format!("{ctor}()");
        }
        return format!("{ctor}({expr})");
    }

    match lang {
        Language::Rust => {
            if expr.is_empty() {
                format!("{default_base}::new()")
            } else {
                format!("{default_base}::new({expr})")
            }
        }
        Language::TypeScript | Language::JavaScript => {
            if expr.is_empty() {
                format!("new {default_base}()")
            } else {
                format!("new {default_base}({expr})")
            }
        }
        Language::Python => {
            if expr.is_empty() {
                format!("{default_base}()")
            } else {
                format!("{default_base}({expr})")
            }
        }
        Language::Cpp | Language::C => {
            if expr.is_empty() {
                format!("{default_base}()")
            } else if !was.is_empty() && was != "void" {
                format!("{default_base}<{was}>({expr})")
            } else {
                format!("{default_base}({expr})")
            }
        }
        Language::Swift => {
            if expr.is_empty() {
                format!("{default_base}()")
            } else {
                format!("{default_base}({expr})")
            }
        }
        Language::Go => {
            let clean_base = default_base.trim_start_matches('*');
            if expr.is_empty() {
                format!("&{clean_base}{{}}")
            } else {
                format!("&{clean_base}{{Data: {expr}}}")
            }
        }
        Language::Java => {
            if expr.is_empty() {
                format!("new {default_base}()")
            } else if !was.is_empty() && was != "void" {
                format!("new {default_base}<>({expr})")
            } else {
                format!("new {default_base}({expr})")
            }
        }
    }
}
