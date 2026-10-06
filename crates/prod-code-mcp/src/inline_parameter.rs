/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Inlining a parameter: when every call passes the same constant for it, the value moves into
//! the body as a local binding, and the parameter leaves the declaration and every call.
//!
//! `fn clamp(x: u32, max: u32)` called as `clamp(v, LIMIT)` everywhere becomes `fn clamp(x: u32)`
//! with `let max: u32 = LIMIT;` at the top of its body, and the calls become `clamp(v)`. The
//! value must mean the same thing in the body as at the call: a literal, a constant, a path. A
//! lowercase name may be a local of the caller and is refused, and so is a set of calls that do
//! not agree on the value. Supports Rust, TypeScript, JavaScript, Python, C++, Swift, and Go.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use crate::parameter_object::Language;

/// What the inlining did, or would do if it were applied.
#[derive(Debug, Clone, serde::Serialize)]
pub struct InlinedParameter {
    pub function: String,
    pub parameter: String,
    /// The value every call passed, now bound at the top of the body.
    pub value: String,
    #[serde(skip)]
    pub root: PathBuf,
    pub file: String,
    pub rewritten_calls: usize,
    /// References that are not a call with this parameter's argument: the function used as a
    /// value, or a call this could not read. They block the write.
    pub unmatched: Vec<String>,
    pub rewritten: Vec<(String, String)>,
    pub diagnostics: Vec<String>,
    pub applied: bool,
}

impl InlinedParameter {
    pub fn render(&self, diff_budget: usize) -> String {
        let mut out = format!(
            "`{}` of `{}` ({})\n\n- every call passes `{}`: it is bound at the top of the body\n- \
             {} call(s) lose the argument\n\n",
            self.parameter, self.function, self.file, self.value, self.rewritten_calls
        );
        let mut body = String::new();
        let mut changed_lines = 0usize;
        for (path, new_text) in &self.rewritten {
            let old_text = crate::refactor::text_before_apply(Path::new(path));
            let rel = display(&self.root, Path::new(path));
            let diff = similar::TextDiff::from_lines(&old_text, new_text);
            changed_lines += diff
                .iter_all_changes()
                .filter(|c| c.tag() != similar::ChangeTag::Equal)
                .count();
            body.push_str(
                &diff
                    .unified_diff()
                    .context_radius(2)
                    .header(&format!("a/{rel}"), &format!("b/{rel}"))
                    .to_string(),
            );
        }
        out.push_str(&format!(
            "{} changed line(s) in {} file(s)\n\n",
            changed_lines,
            self.rewritten.len()
        ));
        if body.len() > diff_budget {
            let cut: String = body.chars().take(diff_budget).collect();
            out.push_str(&cut);
            out.push_str("\n… diff truncated\n");
        } else {
            out.push_str(&body);
        }
        if !self.unmatched.is_empty() {
            out.push_str(&format!(
                "\nnot a call with this argument ({}); nothing is written while any remains:\n",
                self.unmatched.len()
            ));
            for r in &self.unmatched {
                out.push_str(&format!("  {r}\n"));
            }
        }
        if self.diagnostics.is_empty() {
            out.push_str("\nthe analyzer accepts the result: 0 errors\n");
        } else {
            out.push_str("\nthe analyzer rejects the result:\n");
            for d in &self.diagnostics {
                out.push_str(&format!("  {d}\n"));
            }
        }
        if self.applied {
            out.push_str(&format!(
                "\n[applied to {} file(s)]\n",
                self.rewritten.len()
            ));
        } else {
            out.push_str("\nnothing was written; pass `apply: true` to make these edits\n");
        }
        out
    }
}

fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Whether an argument means the same in the callee's body as at the call: a literal, a
/// constant (`ALL_CAPS`), a type or unit value (`Mode`, `None`), or a path of those
/// (`Mode::Fast`, `crate::limits::MAX`, `Config.MAX`, `Math.PI`). A lowercase name may be a local of the caller; a call or
/// an expression may depend on one.
pub fn is_caller_independent(arg: &str) -> bool {
    let a = arg.trim();
    if a.is_empty() {
        return false;
    }
    // Disallow calls, indexings, expressions, borrows
    if a.contains('(')
        || a.contains(')')
        || a.contains('[')
        || a.contains(']')
        || a.contains('+')
        || a.contains('*')
        || a.contains('/')
        || a.contains('&')
        || a.contains('|')
        || a.contains('^')
        || a.contains('~')
        || a.contains('?')
    {
        return false;
    }
    if a.starts_with("self.")
        || a.starts_with("this.")
        || a.starts_with("self->")
        || a.starts_with("this->")
    {
        return false;
    }
    // Numbers
    let literal = a.strip_prefix('-').unwrap_or(a);
    if literal.starts_with(|c: char| c.is_ascii_digit())
        && literal.chars().all(|c| is_ident(c) || c == '.')
    {
        return true;
    }
    // Booleans & Nulls / Units
    if matches!(
        a,
        "true" | "false" | "True" | "False" | "None" | "nil" | "null" | "nullptr" | "undefined" | "Default"
    ) {
        return true;
    }
    // Strings & Chars
    if ((a.starts_with('"') && a.ends_with('"'))
        || (a.starts_with('\'') && a.ends_with('\''))
        || (a.starts_with("b\"") && a.ends_with('"')))
        && !a.contains('{')
    {
        return true;
    }
    // Path / Qualified names: separated by `::` or `.`
    let segments: Vec<&str> = if a.contains("::") {
        a.split("::").collect()
    } else {
        a.split('.').collect()
    };
    if segments.iter().any(|s| s.is_empty() || !s.chars().all(is_ident)) {
        return false;
    }
    if segments.first().is_some_and(|&first| matches!(first, "self" | "this" | "super")) {
        return false;
    }
    let last = segments.last().copied().unwrap_or("");
    let first_char_upper = last.chars().next().is_some_and(char::is_uppercase);
    let all_caps = last
        .chars()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');

    all_caps || first_char_upper
}

fn keyword_arg(arg: &str) -> Option<(&str, &str)> {
    let trimmed = arg.trim();
    let name_len = trimmed.bytes().take_while(|b| is_ident(*b as char)).count();
    if name_len == 0 || trimmed.as_bytes()[0].is_ascii_digit() {
        return None;
    }
    let name = &trimmed[..name_len];
    let rest = trimmed[name_len..].trim_start();
    let value = rest.strip_prefix('=')?;
    if value.starts_with('=') {
        return None;
    }
    Some((name, value.trim()))
}

fn swift_label(arg: &str) -> Option<(&str, &str)> {
    let trimmed = arg.trim();
    let name_len = trimmed.bytes().take_while(|b| is_ident(*b as char)).count();
    if name_len == 0 || trimmed.as_bytes()[0].is_ascii_digit() {
        return None;
    }
    let name = &trimmed[..name_len];
    let rest = trimmed[name_len..].trim_start();
    let value = rest.strip_prefix(':')?;
    Some((name, value.trim()))
}

pub(crate) fn language_matches(lang: Language, path: &Path) -> bool {
    Language::of(path) == Some(lang)
        || (lang == Language::TypeScript && Language::of(path) == Some(Language::JavaScript))
        || (lang == Language::JavaScript && Language::of(path) == Some(Language::TypeScript))
        || (lang == Language::Cpp && Language::of(path) == Some(Language::C))
        || (lang == Language::C && Language::of(path) == Some(Language::Cpp))
}

struct PolyglotDecl {
    fn_name: String,
    open_paren: usize,
    close_paren: usize,
    body_open: usize,
    body_close: usize,
    receiver: Option<String>,
    params: Vec<crate::parameter_object::Param>,
}

pub(crate) fn extract_decl_name_from_line(line: &str, lang: Language) -> Option<String> {
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
                    if clean.chars().all(is_ident) && !clean.is_empty() && clean != "if" && clean != "while" && clean != "for" && clean != "switch" {
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
                    if member.chars().all(is_ident) && !member.is_empty() && member != "if" && member != "while" && member != "for" && member != "switch" {
                        return Some(member.to_string());
                    }
                }
            }
        }
        Language::Rust => {}
    }
    None
}

pub(crate) fn find_python_body_close(text: &str, def_offset: usize, colon_pos: usize) -> usize {
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

fn python_insertion_offset_and_indent(text: &str, colon_pos: usize, def_offset: usize) -> (usize, String) {
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

fn brace_insertion_offset_and_indent(text: &str, body_open: usize, body_close: usize, lang: Language) -> (usize, String) {
    let body_text = &text[body_open + 1..body_close];
    for line in body_text.lines() {
        if !line.trim().is_empty() {
            let indent = line[..line.len() - line.trim_start().len()].to_string();
            return (body_open + 1, indent);
        }
    }
    let default_indent = if lang == Language::Go { "\t".to_string() } else { "    ".to_string() };
    (body_open + 1, default_indent)
}

fn format_binding(param_name: &str, param_type: Option<&str>, val: &str, lang: Language) -> String {
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

fn find_polyglot_declaration(
    text: &str,
    lang: Language,
    line: Option<u32>,
    function: Option<&str>,
) -> Result<PolyglotDecl> {
    let clean_name = function.map(|f| {
        f.rsplit_once("::")
            .map(|(_, m)| m)
            .or_else(|| f.rsplit_once('.').map(|(_, m)| m))
            .unwrap_or(f)
            .trim()
            .to_string()
    }).or_else(|| {
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
    }).context("could not determine function name to inline parameter from")?;

    // Search for declaration of `clean_name`
    let needle_paren = format!("{clean_name}(");
    let needle_space_paren = format!("{clean_name} (");
    let needle_generic = format!("{clean_name}<");

    let mut found_decl = None;
    for (pos, _) in text.match_indices(&needle_paren)
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

        let (receiver, params) = crate::parameter_object::parse_params(&text[open_paren + 1..close_paren], lang);
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

struct FoundCall {
    args_start: usize,
    args_end: usize,
    arg_index: usize,
    passed_value: String,
    site: String,
}

pub(crate) fn is_import_or_export_context(content: &str, at: usize, lang: Language) -> bool {
    let line_start = content[..at].rfind('\n').map_or(0, |p| p + 1);
    let line_end = content[at..].find('\n').map_or(content.len(), |p| at + p);
    let line = content[line_start..line_end].trim();

    match lang {
        Language::Python => {
            line.starts_with("import ") || line.starts_with("from ")
        }
        Language::TypeScript | Language::JavaScript => {
            if line.starts_with("import ")
                || line.starts_with("import{")
                || line.starts_with("export ")
                || line.starts_with("export{")
                || line.contains(" from ")
                || line.contains("require(")
            {
                return true;
            }
            let search_start = at.saturating_sub(500);
            let before = &content[search_start..at];
            if let Some(imp_pos) = before.rfind("import ").or_else(|| before.rfind("export ")) {
                let between = &before[imp_pos..];
                if between.contains('{') && !between.contains('}') {
                    let search_end = content.len().min(at + 500);
                    let after = &content[at..search_end];
                    if after.contains('}') {
                        return true;
                    }
                }
            }
            false
        }
        Language::Go => {
            if line.starts_with("import ") {
                return true;
            }
            let search_start = at.saturating_sub(500);
            let before = &content[search_start..at];
            if let Some(imp_pos) = before.rfind("import (") {
                let between = &before[imp_pos..];
                if !between.contains(')') {
                    let search_end = content.len().min(at + 500);
                    let after = &content[at..search_end];
                    if after.contains(')') {
                        return true;
                    }
                }
            }
            false
        }
        Language::Cpp | Language::C => {
            line.starts_with("#include") || line.starts_with("using ")
        }
        Language::Swift => {
            line.starts_with("import ")
        }
        Language::Rust => {
            let without_pub = line.strip_prefix("pub ")
                .or_else(|| line.strip_prefix("pub(crate) "))
                .unwrap_or(line);
            without_pub.starts_with("use ")
        }
        Language::Java => {
            line.starts_with("import ") || line.starts_with("package ")
        }
    }
}

pub(crate) fn is_in_comment(content: &str, at: usize, lang: Language) -> bool {
    let line_start = content[..at].rfind('\n').map_or(0, |p| p + 1);
    let before_on_line = &content[line_start..at];
    let trimmed = before_on_line.trim_start();
    if lang == Language::Python {
        trimmed.starts_with('#') || before_on_line.contains('#')
    } else {
        trimmed.starts_with("//")
            || trimmed.starts_with("/*")
            || trimmed.starts_with('*')
            || before_on_line.contains("//")
    }
}

pub(crate) fn is_c_cpp_prototype(content: &str, at: usize, close_paren: usize) -> bool {
    let line_start = content[..at].rfind('\n').map_or(0, |p| p + 1);
    let before_on_line = content[line_start..at].trim();
    let Some(before_word) = before_on_line.split_whitespace().last() else {
        return false;
    };
    let clean = before_word
        .trim_start_matches('*')
        .trim_start_matches('&')
        .trim_end_matches('*')
        .trim_end_matches('&');
    if clean.is_empty()
        || matches!(
            clean,
            "return" | "throw" | "case" | "goto" | "new" | "delete" | "co_return" | "co_yield"
        )
    {
        return false;
    }
    if !clean.chars().all(is_ident) {
        return false;
    }
    if close_paren + 1 > content.len() {
        return false;
    }
    let after_paren = content[close_paren + 1..].trim_start();
    after_paren.starts_with(';')
}

pub(crate) fn is_in_string(content: &str, at: usize, lang: Language) -> bool {
    let mut chars = content[..at].chars().peekable();
    let mut quote = None;
    let mut escaped = false;
    let mut line_comment = false;
    let mut block_comment = false;
    while let Some(ch) = chars.next() {
        if line_comment {
            if ch == '\n' {
                line_comment = false;
            }
            continue;
        }
        if block_comment {
            if ch == '*' && chars.peek() == Some(&'/') {
                chars.next();
                block_comment = false;
            }
            continue;
        }
        if let Some(delimiter) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == delimiter {
                quote = None;
            }
            continue;
        }
        if lang == Language::Python && ch == '#' {
            line_comment = true;
        } else if ch == '/' && chars.peek() == Some(&'/') {
            chars.next();
            line_comment = true;
        } else if ch == '/' && chars.peek() == Some(&'*') {
            chars.next();
            block_comment = true;
        } else if matches!(ch, '\'' | '"' | '`') {
            quote = Some(ch);
        }
    }
    quote.is_some() || line_comment || block_comment
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn find_calls_in_content(
    content: &str,
    file_rel: &str,
    fn_name: &str,
    target_param_name: &str,
    target_param_index: usize,
    target_label: Option<&str>,
    has_receiver: bool,
    lang: Language,
    is_decl_file: bool,
    decl_open: usize,
    decl_close: usize,
    body_open: usize,
    body_close: usize,
    selected_param_types: &[Option<String>],
    semantic_references: &mut std::collections::HashSet<(u32, u32)>,
    references_were_empty: bool,
) -> (Vec<FoundCall>, Vec<(usize, usize, String)>, Vec<String>) {
    let mut calls = Vec::new();
    let mut proto_edits = Vec::new();
    let mut unmatched = Vec::new();

    for (at, _) in content.match_indices(fn_name) {
        if at > 0 {
            let prev = content[..at].chars().next_back().unwrap();
            if is_ident(prev) {
                continue;
            }
        }
        let after = &content[at + fn_name.len()..];
        if after.starts_with(is_ident) {
            continue;
        }
        if is_in_comment(content, at, lang) || is_in_string(content, at, lang) {
            continue;
        }

        // Line and column for site reporting
        let (lsp_line, lsp_col) = crate::signature::position_at(content, at).unwrap_or((0, 0));
        let line = lsp_line.saturating_sub(1);
        let col = lsp_col;
        let site = format!("{file_rel}:{line}:{col}");

        if is_import_or_export_context(content, at, lang) {
            semantic_references.remove(&(lsp_line, lsp_col));
            continue;
        }

        // Declaration check
        if is_decl_file && at >= decl_open.saturating_sub(fn_name.len() + 20) && at <= decl_close {
            continue;
        }

        // Self-call check inside function's own body
        if is_decl_file && at > body_open && at < body_close {
            if semantic_references.remove(&(lsp_line, lsp_col)) || references_were_empty {
                unmatched.push(format!("{site} (a call inside `{fn_name}` itself passes its own `{target_param_name}`)"));
            }
            continue;
        }

        let Some((args_start, args_end)) = crate::parameter_object::call_args_span(content, at + fn_name.len()) else {
            if semantic_references.remove(&(lsp_line, lsp_col)) || references_were_empty {
                unmatched.push(format!("{site} (the function used as a value: it would change type)"));
            }
            continue;
        };

        if matches!(lang, Language::Cpp | Language::C) && is_c_cpp_prototype(content, at, args_end) {
            let (_, proto_params) = crate::parameter_object::parse_params(&content[args_start..args_end], lang);
            let proto_types = proto_params.iter().map(|p| p.ty.clone()).collect::<Vec<_>>();
            if proto_types == selected_param_types
                && let Some(p_idx) = proto_params.iter().position(|p| p.name == target_param_name)
            {
                let kept: Vec<String> = proto_params.iter().enumerate().filter(|(i, _)| *i != p_idx).map(|(_, p)| p.raw.clone()).collect();
                proto_edits.push((args_start, args_end - args_start, kept.join(", ")));
            }
            continue;
        }

        if !semantic_references.remove(&(lsp_line, lsp_col)) {
            if references_were_empty {
                unmatched.push(format!("{site} (analyzer references for `{fn_name}` were empty)"));
            }
            continue;
        }

        let args_str = &content[args_start..args_end];
        let args = crate::parameter_object::split_args(args_str);

        let mut found_idx = None;
        let mut found_val = None;

        if lang == Language::Python {
            for (i, a) in args.iter().enumerate() {
                if let Some((k, v)) = keyword_arg(a)
                    && k == target_param_name
                {
                    found_idx = Some(i);
                    found_val = Some(v.to_string());
                    break;
                }
            }
            if found_idx.is_none() {
                let before = content[..at].trim_end();
                let is_method = before.ends_with('.');
                let pos = if has_receiver && !is_method {
                    target_param_index + 1
                } else {
                    target_param_index
                };
                if let Some(a) = args.get(pos) {
                    found_idx = Some(pos);
                    found_val = Some(a.trim().to_string());
                }
            }
        } else if lang == Language::Swift {
            for (i, a) in args.iter().enumerate() {
                if let Some((lbl, v)) = swift_label(a)
                    && (lbl == target_param_name || target_label == Some(lbl))
                {
                    found_idx = Some(i);
                    found_val = Some(v.to_string());
                    break;
                }
            }
            if found_idx.is_none() && let Some(a) = args.get(target_param_index) {
                found_idx = Some(target_param_index);
                found_val = Some(a.trim().to_string());
            }
        } else if let Some(a) = args.get(target_param_index) {
            found_idx = Some(target_param_index);
            found_val = Some(a.trim().to_string());
        }

        if let (Some(idx), Some(val)) = (found_idx, found_val) {
            calls.push(FoundCall {
                args_start,
                args_end,
                arg_index: idx,
                passed_value: val,
                site,
            });
        } else {
            unmatched.push(format!("{site} (the call has no argument for `{target_param_name}`)"));
        }
    }

    (calls, proto_edits, unmatched)
}

/// Inlines a parameter in polyglot languages: TypeScript/JavaScript, Python, C++, Swift, Go.
#[allow(clippy::too_many_arguments)]
pub async fn inline_parameter_polyglot(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: Option<u32>,
    character: Option<u32>,
    function: Option<&str>,
    param: Option<&str>,
    apply: bool,
    force: bool,
) -> Result<InlinedParameter> {
    let lang = Language::of(file).with_context(|| format!("unsupported language for {}", file.display()))?;
    let text = std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;

    let decl = find_polyglot_declaration(&text, lang, line, function)?;
    let name_at = text[..decl.open_paren]
        .rfind(&decl.fn_name)
        .context("cannot locate the selected function name")?;
    let (reference_line, reference_col) = crate::signature::position_at(&text, name_at)?;
    let mut references_by_file: BTreeMap<PathBuf, std::collections::HashSet<(u32, u32)>> =
        BTreeMap::new();
    for (path, ref_line, ref_col) in crate::signature::references(
        remote,
        root,
        file,
        reference_line,
        reference_col,
    )
    .await?
    {
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        references_by_file
            .entry(path)
            .or_default()
            .insert((ref_line, ref_col));
    }
    let references_were_empty = references_by_file.values().all(|references| references.is_empty());
    let target_idx = if let Some(p_name) = param {
        decl.params
            .iter()
            .position(|p| p.name == p_name)
            .with_context(|| format!("parameter `{p_name}` not found in `{}`", decl.fn_name))?
    } else if let Some(c) = character && let Some(l) = line {
        let offset = crate::signature::offset_of(&text, l, c)
            .context("the parameter position is not inside the file")?;
        decl.params
            .iter()
            .position(|p| {
                let start = decl.open_paren + 1 + p.name_at;
                start <= offset && offset <= start + p.name.len()
            })
            .context("the selected position is not inside a parameter name")?
    } else if decl.params.len() == 1 {
        0
    } else {
        anyhow::bail!("multiple parameters in `{}`; specify which parameter to inline using `parameter`", decl.fn_name);
    };

    let target_param = &decl.params[target_idx];
    let target_param_name = target_param.name.clone();
    let target_param_type = target_param.ty.clone();
    let target_label = target_param.label.clone();
    let has_receiver = decl.receiver.is_some();
    let selected_param_types = decl.params.iter().map(|param| param.ty.clone()).collect::<Vec<_>>();

    let mut texts: BTreeMap<PathBuf, String> = BTreeMap::new();
    texts.insert(file.to_path_buf(), text.clone());
    let mut edits: BTreeMap<PathBuf, Vec<(usize, usize, String)>> = BTreeMap::new();
    let mut all_unmatched = Vec::new();
    let mut values: Vec<(String, String)> = Vec::new();

    let rel_decl_file = display(root, file);
    let canonical_decl = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let mut decl_references = references_by_file.remove(&canonical_decl).unwrap_or_default();
    let (decl_calls, decl_protos, decl_unmatched) = find_calls_in_content(
        &text,
        &rel_decl_file,
        &decl.fn_name,
        &target_param_name,
        target_idx,
        target_label.as_deref(),
        has_receiver,
        lang,
        true,
        decl.open_paren,
        decl.close_paren,
        decl.body_open,
        decl.body_close,
        &selected_param_types,
        &mut decl_references,
        references_were_empty,
    );
    all_unmatched.extend(decl_unmatched);
    for (ref_line, ref_col) in decl_references {
        all_unmatched.push(format!(
            "{rel_decl_file}:{ref_line}:{ref_col}: analyzer reference could not be rewritten safely"
        ));
    }
    for (p_at, p_len, p_rep) in decl_protos {
        edits.entry(file.to_path_buf()).or_default().push((p_at, p_len, p_rep));
    }
    for call in decl_calls {
        values.push((call.site, call.passed_value));
        let args_str = &text[call.args_start..call.args_end];
        let args = crate::parameter_object::split_args(args_str);
        let remaining: Vec<&str> = args
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != call.arg_index)
            .map(|(_, a)| a.trim())
            .collect();
        edits.entry(file.to_path_buf()).or_default().push((
            call.args_start,
            call.args_end - call.args_start,
            remaining.join(", "),
        ));
    }

    // Search workspace files for calls
    for entry in ignore::WalkBuilder::new(root).build().flatten() {
        let path = entry.path();
        if !path.is_file() || path == file || !language_matches(lang, path) {
            continue;
        }
        let Ok(other_content) = std::fs::read_to_string(path) else {
            continue;
        };
        if !other_content.contains(&decl.fn_name) {
            continue;
        }
        let rel_other = display(root, path);
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let mut other_references = references_by_file.remove(&canonical).unwrap_or_default();
        let (other_calls, other_protos, other_unmatched) = find_calls_in_content(
            &other_content,
            &rel_other,
            &decl.fn_name,
            &target_param_name,
            target_idx,
            target_label.as_deref(),
            has_receiver,
            lang,
            false,
            0,
            0,
            0,
            0,
            &selected_param_types,
            &mut other_references,
            references_were_empty,
        );
        all_unmatched.extend(other_unmatched);
        for (ref_line, ref_col) in other_references {
            all_unmatched.push(format!(
                "{rel_other}:{ref_line}:{ref_col}: analyzer reference could not be rewritten safely"
            ));
        }
        if !other_calls.is_empty() || !other_protos.is_empty() {
            texts.insert(path.to_path_buf(), other_content.clone());
            for (p_at, p_len, p_rep) in other_protos {
                edits.entry(path.to_path_buf()).or_default().push((p_at, p_len, p_rep));
            }
            for call in other_calls {
                values.push((call.site, call.passed_value));
                let args_str = &other_content[call.args_start..call.args_end];
                let args = crate::parameter_object::split_args(args_str);
                let remaining: Vec<&str> = args
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i != call.arg_index)
                    .map(|(_, arg)| arg.trim())
                    .collect();
                edits.entry(path.to_path_buf()).or_default().push((
                    call.args_start,
                    call.args_end - call.args_start,
                    remaining.join(", "),
                ));
            }
        }
    }

    for (path, references) in references_by_file {
        for (ref_line, ref_col) in references {
            all_unmatched.push(format!(
                "{}:{ref_line}:{ref_col}: analyzer reference could not be rewritten safely",
                display(root, &path)
            ));
        }
    }

    let first = values.first().map(|(_, v)| v.clone()).with_context(|| {
        format!("no call passes a value for `{target_param_name}`, so there is none to inline")
    })?;
    let differing: Vec<String> = values
        .iter()
        .filter(|(_, v)| *v != first)
        .map(|(site, v)| format!("{site} passes `{v}`"))
        .collect();
    anyhow::ensure!(
        differing.is_empty(),
        "the calls do not agree on `{target_param_name}`: {} of {} pass `{first}`, and\n  {}",
        values.len() - differing.len(),
        values.len(),
        differing.join("\n  ")
    );
    anyhow::ensure!(
        is_caller_independent(&first),
        "every call passes `{first}` for `{target_param_name}`, but it may name something of the caller's (a local, or an expression over one); only a literal, a constant or a path is inlined"
    );

    // Declaration edits in declaring file
    let mut kept_params = Vec::new();
    if let Some(r) = &decl.receiver {
        kept_params.push(r.clone());
    }
    for (i, p) in decl.params.iter().enumerate() {
        if i != target_idx {
            if lang == Language::Go && p.shares_type {
                if let Some(ty) = &p.ty {
                    kept_params.push(format!("{} {}", p.name, ty));
                } else {
                    kept_params.push(p.raw.clone());
                }
            } else {
                kept_params.push(p.raw.clone());
            }
        }
    }

    let (insert_offset, indent) = if lang == Language::Python {
        python_insertion_offset_and_indent(&text, decl.body_open, decl.open_paren)
    } else {
        brace_insertion_offset_and_indent(&text, decl.body_open, decl.body_close, lang)
    };

    let binding = format_binding(&target_param_name, target_param_type.as_deref(), &first, lang);
    let insertion_text = if lang == Language::Python {
        format!("{indent}{binding}\n")
    } else {
        format!("\n{indent}{binding}")
    };

    let own_edits = edits.entry(file.to_path_buf()).or_default();
    own_edits.push((
        decl.open_paren + 1,
        decl.close_paren - (decl.open_paren + 1),
        kept_params.join(", "),
    ));
    own_edits.push((insert_offset, 0, insertion_text));

    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    for (path, mut file_edits) in edits {
        let mut body = texts.get(&path).cloned().unwrap_or_default();
        file_edits.sort_by_key(|(at, len, _)| (*at, *len != 0));
        for (at, len, replacement) in file_edits.into_iter().rev() {
            body.replace_range(at..at + len, &replacement);
        }
        rewritten.insert(path, body);
    }
    let rewritten_calls = values.len();

    let to_check: Vec<(PathBuf, String)> = rewritten
        .iter()
        .map(|(p, t)| (p.clone(), t.clone()))
        .collect();
    let reports = crate::diagnostics::validate_texts(remote, root, &to_check, &[]).await?;
    let diagnostics: Vec<String> = reports
        .iter()
        .flat_map(|r| r.items.iter().map(move |d| (r.file.clone(), d)))
        .filter(|(_, d)| d.severity == "error")
        .map(|(f, d)| {
            format!(
                "{}{} ({f}:{}:{})",
                d.message.lines().next().unwrap_or(""),
                d.code
                    .as_deref()
                    .map(|c| format!(" [{c}]"))
                    .unwrap_or_default(),
                d.line,
                d.col
            )
        })
        .collect();

    let mut applied = false;
    if apply {
        anyhow::ensure!(
            all_unmatched.is_empty(),
            "{} reference(s) to `{}` are not a call passing `{target_param_name}`; nothing was written:\n  {}",
            all_unmatched.len(),
            decl.fn_name,
            all_unmatched.join("\n  ")
        );
        anyhow::ensure!(
            diagnostics.is_empty() || force,
            "the change does not compile ({} error(s)); nothing was written. Pass `force: true` to write it anyway:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        let edit = crate::signature::whole_file_edit(&rewritten);
        crate::refactor::apply_workspace_edit(root, &edit)?;
        applied = true;
    }

    Ok(InlinedParameter {
        function: decl.fn_name,
        parameter: target_param_name,
        value: first,
        root: root.to_path_buf(),
        file: rel_decl_file,
        rewritten_calls,
        unmatched: all_unmatched,
        rewritten: rewritten
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        diagnostics,
        applied,
    })
}

/// Inlines the parameter at `line`:`col` of `file`.
#[allow(clippy::too_many_arguments)]
pub async fn inline_parameter(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    apply: bool,
    force: bool,
) -> Result<InlinedParameter> {
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let (fn_at, param, _) = crate::signature::parameter_at(&text, line, col)
        .context("the position is not on a parameter of a function declaration")?;
    let (function, open, close) =
        crate::signature::param_span(&text, fn_at).context("the function has no parameter list")?;
    let (receiver, declared) = crate::signature::parse_declared(&text[open..close]);
    let index = declared
        .iter()
        .position(|d| d.name == param)
        .context("the parameter is not in the list")?;
    let raw = declared[index].raw.clone();
    anyhow::ensure!(
        raw.contains(':'),
        "`{param}` has no type written; a `let` for it needs one"
    );
    let body_open = text[close..]
        .find('{')
        .map(|i| close + i)
        .with_context(|| format!("`{function}` has no body"))?;
    let body_close = crate::parameter_object::matching_bracket(&text, body_open)
        .context("the function's body does not close")?;

    let mut texts: BTreeMap<PathBuf, String> = BTreeMap::new();
    texts.insert(file.to_path_buf(), text.clone());
    let mut edits: BTreeMap<PathBuf, Vec<(usize, usize, String)>> = BTreeMap::new();
    let mut unmatched = Vec::new();
    let mut values: Vec<(String, String)> = Vec::new();
    let (fl, fc) = crate::signature::position_at(&text, fn_at)?;
    let canonical = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let refs = crate::signature::references(remote, root, file, fl, fc)
        .await
        .with_context(|| format!("cannot find the calls to `{function}`; nothing was planned"))?;
    for (path, l, c) in refs {
        let body = crate::refactor::referenced_text(&mut texts, &path)?.clone();
        let site = format!("{}:{l}:{c}", display(root, &path));
        let Some(at) = crate::signature::offset_of(&body, l, c) else {
            unmatched.push(format!("{site} (the position is not in the file)"));
            continue;
        };
        if !body[at..].starts_with(function.as_str())
            || body[at + function.len()..].starts_with(is_ident)
        {
            unmatched.push(format!(
                "{site} (the analyzer places `{function}` here, but the file says otherwise)"
            ));
            continue;
        }
        let same_file = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone()) == canonical;
        if same_file && body_open < at && at < body_close {
            unmatched.push(format!(
                "{site} (a call inside `{function}` itself passes its own `{param}`)"
            ));
            continue;
        }
        let Some((args_start, args_end)) =
            crate::parameter_object::call_args_span(&body, at + function.len())
        else {
            unmatched.push(format!(
                "{site} (the function used as a value: it would change type)"
            ));
            continue;
        };
        let args = crate::parameter_object::split_args(&body[args_start..args_end]);
        let method_syntax = body[..at].trim_end().ends_with('.');
        let arg_index = if receiver.is_some() && !method_syntax {
            index + 1
        } else {
            index
        };
        let Some(arg) = args.get(arg_index) else {
            unmatched.push(format!("{site} (the call has no argument for `{param}`)"));
            continue;
        };
        values.push((site, arg.trim().to_string()));
        let remaining: Vec<&str> = args
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != arg_index)
            .map(|(_, a)| a.trim())
            .collect();
        edits.entry(path.clone()).or_default().push((
            args_start,
            args_end - args_start,
            remaining.join(", "),
        ));
    }

    let first = values.first().map(|(_, v)| v.clone()).with_context(|| {
        format!("no call passes a value for `{param}`, so there is none to inline")
    })?;
    let differing: Vec<String> = values
        .iter()
        .filter(|(_, v)| *v != first)
        .map(|(site, v)| format!("{site} passes `{v}`"))
        .collect();
    anyhow::ensure!(
        differing.is_empty(),
        "the calls do not agree on `{param}`: {} of {} pass `{first}`, and\n  {}",
        values.len() - differing.len(),
        values.len(),
        differing.join("\n  ")
    );
    anyhow::ensure!(
        is_caller_independent(&first),
        "every call passes `{first}` for `{param}`, but it may name something of the caller's \
         (a local, or an expression over one); only a literal, a constant or a path is inlined"
    );

    // The declaration: without the parameter, and the value bound at the top of the body.
    let mut kept: Vec<String> = receiver.into_iter().collect();
    kept.extend(
        declared
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != index)
            .map(|(_, d)| d.raw.clone()),
    );
    let first_line_indent = text[body_open + 1..]
        .lines()
        .find(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .unwrap_or(4);
    let own = edits.entry(file.to_path_buf()).or_default();
    own.push((open, close - open, kept.join(", ")));
    own.push((
        body_open + 1,
        0,
        format!("\n{}let {raw} = {first};", " ".repeat(first_line_indent)),
    ));

    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    for (path, mut file_edits) in edits {
        let mut body = texts.get(&path).cloned().unwrap_or_default();
        file_edits.sort_by_key(|(at, len, _)| (*at, *len != 0));
        for (at, len, replacement) in file_edits.into_iter().rev() {
            body.replace_range(at..at + len, &replacement);
        }
        rewritten.insert(path, body);
    }
    let rewritten_calls = values.len();

    let to_check: Vec<(PathBuf, String)> = rewritten
        .iter()
        .map(|(p, t)| (p.clone(), t.clone()))
        .collect();
    let reports = crate::diagnostics::validate_texts(remote, root, &to_check, &[]).await?;
    let diagnostics: Vec<String> = reports
        .iter()
        .flat_map(|r| r.items.iter().map(move |d| (r.file.clone(), d)))
        .filter(|(_, d)| d.severity == "error")
        .map(|(f, d)| {
            format!(
                "{}{} ({f}:{}:{})",
                d.message.lines().next().unwrap_or(""),
                d.code
                    .as_deref()
                    .map(|c| format!(" [{c}]"))
                    .unwrap_or_default(),
                d.line,
                d.col
            )
        })
        .collect();

    let mut applied = false;
    if apply {
        anyhow::ensure!(
            unmatched.is_empty(),
            "{} reference(s) to `{function}` are not a call passing `{param}`; nothing was \
             written:\n  {}",
            unmatched.len(),
            unmatched.join("\n  ")
        );
        anyhow::ensure!(
            diagnostics.is_empty() || force,
            "the change does not compile ({} error(s)); nothing was written. Pass `force: true` \
             to write it anyway:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        let edit = crate::signature::whole_file_edit(&rewritten);
        crate::refactor::apply_workspace_edit(root, &edit)?;
        applied = true;
    }

    Ok(InlinedParameter {
        function,
        parameter: param,
        value: first,
        root: root.to_path_buf(),
        file: display(root, file),
        rewritten_calls,
        unmatched,
        rewritten: rewritten
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        diagnostics,
        applied,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_value_that_means_the_same_in_the_body_is_inlined() {
        for ok in [
            "10",
            "-3",
            "2.5",
            "1_000u64",
            "true",
            "false",
            "True",
            "False",
            "None",
            "nil",
            "null",
            "nullptr",
            "undefined",
            "\"x\"",
            "b\"raw\"",
            "'c'",
            "LIMIT",
            "Mode::Fast",
            "crate::limits::MAX",
            "Config.MAX",
            "Math.PI",
            "Default",
        ] {
            assert!(is_caller_independent(ok), "{ok}");
        }
        for no in [
            "limit",
            "v + 1",
            "f()",
            "self.max",
            "this.max",
            "&x",
            "LIMIT + 1",
            "format!(\"{x}\")",
            "\"{x}\"",
            "Mode::from(x)",
            "self.MAX",
            "this.LIMIT",
        ] {
            assert!(!is_caller_independent(no), "{no}");
        }
    }

    #[test]
    fn the_report_names_the_value_and_the_calls() {
        let done = InlinedParameter {
            function: "clamp".into(),
            parameter: "max".into(),
            value: "LIMIT".into(),
            root: "/nonexistent".into(),
            file: "src/lib.rs".into(),
            rewritten_calls: 2,
            unmatched: vec![
                "src/lib.rs:9:5 (the function used as a value: it would change type)".into(),
            ],
            rewritten: vec![],
            diagnostics: vec![],
            applied: false,
        };
        let text = done.render(1000);
        assert!(text.contains("every call passes `LIMIT`"), "{text}");
        assert!(text.contains("2 call(s) lose the argument"), "{text}");
        assert!(
            text.contains("nothing is written while any remains"),
            "{text}"
        );
        assert!(text.contains("nothing was written"), "{text}");
    }

    #[test]
    fn polyglot_format_binding_generates_correct_syntax() {
        assert_eq!(
            format_binding("max", Some("number"), "100", Language::TypeScript),
            "const max: number = 100;"
        );
        assert_eq!(
            format_binding("max", None, "100", Language::JavaScript),
            "const max = 100;"
        );
        assert_eq!(
            format_binding("max", Some("int"), "100", Language::Python),
            "max = 100"
        );
        assert_eq!(
            format_binding("max", Some("int"), "100", Language::Cpp),
            "const int max = 100;"
        );
        assert_eq!(
            format_binding("max", Some("Int"), "100", Language::Swift),
            "let max: Int = 100"
        );
        assert_eq!(
            format_binding("max", Some("int"), "100", Language::Go),
            "const max = 100"
        );
        assert_eq!(
            format_binding("ptr", None, "&MyStruct{}", Language::Go),
            "var ptr = &MyStruct{}"
        );
    }

    #[test]
    fn polyglot_keyword_arg_and_swift_label_parse_correctly() {
        assert_eq!(keyword_arg("max=100"), Some(("max", "100")));
        assert_eq!(keyword_arg("max = 100"), Some(("max", "100")));
        assert_eq!(keyword_arg("a == b"), None);
        assert_eq!(keyword_arg("100"), None);

        assert_eq!(swift_label("max: 100"), Some(("max", "100")));
        assert_eq!(swift_label("label: val"), Some(("label", "val")));
        assert_eq!(swift_label("100"), None);
    }
}
