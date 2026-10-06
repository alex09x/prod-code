/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Polyglot change signature across TypeScript/JavaScript, Python, C++, and Swift (Roadmap 7.1.1).
//!
//! Reorders, adds, and removes parameters; modifies return types, visibility, and async modifiers;
//! updates declarations and header prototypes, and rewrites all workspace call sites while
//! preserving argument expressions, keyword arguments, and labels.

use anyhow::{Context, Result};
use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use crate::parameter_object::{Language, call_args_span, matching_bracket, split_args};
use crate::signature::{Modifiers, Param, SignatureChange};

#[derive(Debug, Clone)]
struct PolyglotDecl {
    fn_name: String,
    open_paren: usize,
    close_paren: usize,
    body_open: usize,
    body_close: usize,
    receiver: Option<String>,
    params: Vec<crate::parameter_object::Param>,
    ret_span: Option<(usize, usize)>,
    is_async: bool,
    async_keyword_span: Option<(usize, usize)>,
    visibility_span: Option<(usize, usize)>,
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn is_side_effect_free_argument(argument: &str, lang: Language) -> bool {
    let argument = if lang == Language::Python {
        argument
            .split_once('=')
            .map(|(_, value)| value.trim())
            .unwrap_or(argument.trim())
    } else if lang == Language::Swift {
        argument
            .split_once(':')
            .filter(|(label, _)| label.trim().chars().all(is_ident))
            .map(|(_, value)| value.trim())
            .unwrap_or(argument.trim())
    } else {
        argument.trim()
    };
    if argument.is_empty() {
        return false;
    }
    if argument.chars().all(is_ident) {
        return true;
    }
    if matches!(argument, "true" | "false" | "True" | "False" | "None" | "nil" | "null" | "nullptr") {
        return true;
    }
    let first = argument.chars().next().unwrap_or_default();
    if first.is_ascii_digit() || ((first == '-' || first == '+') && argument.chars().nth(1).is_some_and(|c| c.is_ascii_digit())) {
        return argument
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'));
    }
    if let Some(quote) = argument.chars().next().filter(|c| matches!(c, '\'' | '"')) {
        return argument.ends_with(quote) && !argument[1..argument.len().saturating_sub(1)].contains('\n');
    }
    false
}

fn one_based_lsp_position(text: &str, byte_offset: usize) -> (u32, u32) {
    let before = &text[..byte_offset];
    let line = before.bytes().filter(|byte| *byte == b'\n').count() as u32 + 1;
    let column_text = before.rsplit('\n').next().unwrap_or_default();
    let character = column_text.encode_utf16().count() as u32 + 1;
    (line, character)
}

fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
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

pub(crate) fn is_import_or_export_context(content: &str, at: usize, lang: Language) -> bool {
    let line_start = content[..at].rfind('\n').map_or(0, |p| p + 1);
    let line_end = content[at..].find('\n').map_or(content.len(), |p| at + p);
    let line = content[line_start..line_end].trim();

    match lang {
        Language::Python => line.starts_with("import ") || line.starts_with("from "),
        Language::TypeScript | Language::JavaScript => {
            if line.starts_with("import ")
                || line.starts_with("import{")
                || line.contains(" from ")
                || line.contains("require(")
            {
                return true;
            }
            let search_start = at.saturating_sub(300);
            let before = &content[search_start..at];
            if let Some(imp_pos) = before.rfind("import ") {
                let between = &before[imp_pos..];
                if between.contains('{') && !between.contains('}') {
                    let search_end = content.len().min(at + 300);
                    let after = &content[at..search_end];
                    if after.contains('}') {
                        return true;
                    }
                }
            }
            false
        }
        Language::Cpp | Language::C => line.starts_with("#include") || line.starts_with("using "),
        Language::Swift => line.starts_with("import "),
        Language::Go => line.starts_with("import "),
        Language::Rust => line.starts_with("use "),
        Language::Java => line.starts_with("import ") || line.starts_with("package "),
    }
}

fn is_word_used(body: &str, word: &str) -> bool {
    for (at, _) in body.match_indices(word) {
        if at > 0 {
            let prev = body[..at].chars().next_back().unwrap();
            if is_ident(prev) {
                continue;
            }
        }
        let after = &body[at + word.len()..];
        if after.starts_with(is_ident) {
            continue;
        }
        // Exclude uses inside comments
        let line_start = body[..at].rfind('\n').map_or(0, |p| p + 1);
        let before_on_line = &body[line_start..at];
        if before_on_line.contains("//") || before_on_line.contains('#') {
            continue;
        }
        return true;
    }
    false
}

fn find_python_body_close(text: &str, def_offset: usize, colon_pos: usize) -> usize {
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

fn extract_decl_name_from_line(line: &str, lang: Language) -> Option<String> {
    let trimmed = line.trim();
    match lang {
        Language::Python => {
            if let Some(pos) = trimmed.find("def ") {
                let after = &trimmed[pos + 4..];
                let paren = after.find('(')?;
                let name = after[..paren].trim();
                return Some(name.to_string());
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
                        && !matches!(clean, "if" | "while" | "for" | "switch" | "catch")
                    {
                        return Some(clean.to_string());
                    }
                }
            }
        }
        Language::Go => {
            if let Some(pos) = trimmed.find("func ") {
                let after = &trimmed[pos + 5..];
                let rest = if after.starts_with('(') {
                    if let Some(close_recv) = after.find(')') {
                        after[close_recv + 1..].trim_start()
                    } else {
                        after
                    }
                } else {
                    after
                };
                if let Some(paren) = rest.find('(').or_else(|| rest.find('[')) {
                    let name = rest[..paren].trim();
                    if name.chars().all(is_ident) && !name.is_empty() {
                        return Some(name.to_string());
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
                        && !matches!(clean, "if" | "while" | "for" | "switch" | "catch")
                    {
                        return Some(member.to_string());
                    }
                }
            }
        }
        _ => {}
    }
    None
}

fn find_polyglot_declaration(
    text: &str,
    lang: Language,
    line: u32,
    _col: u32,
) -> Result<PolyglotDecl> {
    let lines: Vec<&str> = text.lines().collect();
    let l_idx = (line.saturating_sub(1) as usize).min(lines.len().saturating_sub(1));

    let mut candidate_name = None;
    if let Some(name) = extract_decl_name_from_line(lines[l_idx], lang) {
        candidate_name = Some(name);
    } else {
        let start_idx = l_idx.saturating_sub(10);
        for line in lines[start_idx..l_idx].iter().rev() {
            if let Some(name) = extract_decl_name_from_line(line, lang) {
                candidate_name = Some(name);
                break;
            }
        }
        if candidate_name.is_none() {
            let end_idx = (l_idx + 4).min(lines.len().saturating_sub(1));
            for line in &lines[l_idx + 1..=end_idx] {
                if let Some(name) = extract_decl_name_from_line(line, lang) {
                    candidate_name = Some(name);
                    break;
                }
            }
        }
    }

    let clean_name = candidate_name.context("could not determine function declaration name")?;

    let needle_paren = format!("{clean_name}(");
    let needle_space_paren = format!("{clean_name} (");
    let needle_generic = format!("{clean_name}<");

    let target_offset = text.lines().take(l_idx).map(|l| l.len() + 1).sum::<usize>();

    let mut candidates = Vec::new();
    for (pos, _) in text
        .match_indices(&needle_paren)
        .chain(text.match_indices(&needle_space_paren))
        .chain(text.match_indices(&needle_generic))
    {
        if pos > 0 {
            let prev = text[..pos].chars().next_back().unwrap();
            if is_ident(prev) {
                continue;
            }
        }
        let line_start = text[..pos].rfind('\n').map_or(0, |p| p + 1);
        let header_prefix = &text[line_start..pos];
        if is_in_comment(text, pos, lang) || is_import_or_export_context(text, pos, lang) {
            continue;
        }

        let is_decl = match lang {
            Language::Python => header_prefix.contains("def "),
            Language::Swift => header_prefix.contains("func "),
            Language::Go => header_prefix.contains("func "),
            Language::TypeScript | Language::JavaScript => {
                header_prefix.contains("function ")
                    || header_prefix.trim_start().starts_with("async ")
                    || (header_prefix.trim().chars().all(is_ident) && !header_prefix.trim().is_empty())
            }
            Language::Cpp | Language::C | Language::Java => {
                let last = header_prefix.split_whitespace().last().unwrap_or("");
                !matches!(last, "return" | "throw" | "case" | "sizeof" | "")
                    && !header_prefix.trim_end().ends_with(['=', '(', '[', ',', '?', ':', '!', '+', '-', '*', '/', '%', '&', '|', '^'])
            }
            _ => true,
        };
        if !is_decl {
            continue;
        }

        let after_name = pos + clean_name.len();
        let open_paren = match text[after_name..].find('(') {
            Some(p) => after_name + p,
            None => continue,
        };
        let close_paren = match matching_bracket(text, open_paren) {
            Some(p) => p,
            None => continue,
        };

        let (body_open, body_close, ret_span) = if lang == Language::Python {
            let colon = match text[close_paren..].find(':') {
                Some(c) => close_paren + c,
                None => continue,
            };
            let b_close = find_python_body_close(text, pos, colon);
            let ret = (close_paren < colon).then_some((close_paren + 1, colon));
            (colon, b_close, ret)
        } else {
            let b_open = match text[close_paren..].find('{') {
                Some(b) => close_paren + b,
                None => continue,
            };
            let b_close = match matching_bracket(text, b_open) {
                Some(b) => b,
                None => continue,
            };
            let ret = match lang {
                Language::TypeScript | Language::JavaScript
                | Language::Swift | Language::Go => Some((close_paren + 1, b_open)),
                Language::Cpp | Language::C | Language::Java => {
                    let line_start = text[..pos].rfind('\n').map_or(0, |p| p + 1);
                    Some((line_start, pos))
                }
                _ => None,
            };
            (b_open, b_close, ret)
        };

        let is_async = header_prefix.contains("async ") || text[close_paren..body_open].contains("async");

        let async_keyword_span = header_prefix
            .find("async ")
            .map(|idx| (line_start + idx, line_start + idx + 6));

        let visibility_span = if let Some(idx) = header_prefix.find("export ") {
            Some((line_start + idx, line_start + idx + 7))
        } else if let Some(idx) = header_prefix.find("public ") {
            Some((line_start + idx, line_start + idx + 7))
        } else {
            header_prefix
                .find("private ")
                .map(|idx| (line_start + idx, line_start + idx + 8))
        };

        let (receiver, params) = crate::parameter_object::parse_params(
            &text[open_paren + 1..close_paren],
            lang,
        );

        let dist = pos.abs_diff(target_offset);
        candidates.push((
            dist,
            PolyglotDecl {
                fn_name: clean_name.clone(),
                open_paren,
                close_paren,
                body_open,
                body_close,
                receiver,
                params,
                ret_span,
                is_async,
                async_keyword_span,
                visibility_span,
            },
        ));
    }

    candidates.sort_by_key(|(d, _)| *d);
    let found_decl = candidates.into_iter().next().map(|(_, decl)| decl);

    found_decl.with_context(|| format!("could not locate declaration for function at line {line}"))
}

fn format_polyglot_param(name: &str, ty: &str, value: &str, lang: Language) -> String {
    match lang {
        Language::TypeScript => {
            if !ty.is_empty() && !value.is_empty() {
                format!("{name}: {ty} = {value}")
            } else if !ty.is_empty() {
                format!("{name}: {ty}")
            } else if !value.is_empty() {
                format!("{name} = {value}")
            } else {
                name.to_string()
            }
        }
        Language::JavaScript => {
            if !value.is_empty() {
                format!("{name} = {value}")
            } else {
                name.to_string()
            }
        }
        Language::Python => {
            if !ty.is_empty() && !value.is_empty() {
                format!("{name}: {ty} = {value}")
            } else if !ty.is_empty() {
                format!("{name}: {ty}")
            } else if !value.is_empty() {
                format!("{name} = {value}")
            } else {
                name.to_string()
            }
        }
        // Added call sites receive `value` explicitly. A C++ default here would be
        // duplicated between a header declaration and its source definition.
        Language::Cpp | Language::C => format!("{ty} {name}"),
        Language::Swift => {
            if !value.is_empty() {
                format!("{name}: {ty} = {value}")
            } else {
                format!("{name}: {ty}")
            }
        }
        Language::Go => format!("{name} {ty}"),
        Language::Rust => {
            if !value.is_empty() {
                format!("{name}: {ty} = {value}")
            } else {
                format!("{name}: {ty}")
            }
        }
        Language::Java => {
            let ty_str = if ty.is_empty() { "Object" } else { ty };
            format!("{ty_str} {name}")
        }
    }
}

pub(crate) fn is_candidate_source_file(path: &Path, lang: Language) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    match lang {
        Language::TypeScript => matches!(ext, "ts" | "tsx" | "js" | "jsx"),
        Language::JavaScript => matches!(ext, "js" | "jsx" | "ts" | "tsx"),
        Language::Python => ext == "py",
        Language::Cpp | Language::C => matches!(ext, "cpp" | "cc" | "cxx" | "c" | "h" | "hpp" | "hxx"),
        Language::Swift => ext == "swift",
        Language::Go => ext == "go",
        Language::Rust => ext == "rs",
        Language::Java => ext == "java",
    }
}

fn is_c_cpp_prototype(content: &str, at: usize, _args_start: usize, args_end: usize) -> bool {
    let after_paren = content[args_end + 1..].trim_start();
    if !after_paren.starts_with(';') {
        return false;
    }

    let line_start = content[..at].rfind('\n').map_or(0, |p| p + 1);
    let before = content[line_start..at].trim();

    // If there is nothing before fn_name on this line, in C/C++ it cannot be a prototype
    // (a prototype must have a return type like `void foo();` or `int foo();`).
    if before.is_empty() {
        return false;
    }

    let last_word = before.split_whitespace().last().unwrap_or("");
    if matches!(
        last_word,
        "return" | "throw" | "case" | "sizeof" | "decltype" | "co_return" | "co_yield"
    ) {
        return false;
    }
    if before.ends_with('=')
        || before.ends_with('(')
        || before.ends_with('[')
        || before.ends_with(',')
        || before.ends_with('?')
        || before.ends_with(':')
        || before.ends_with('!')
        || before.ends_with('+')
        || before.ends_with('-')
        || before.ends_with('*')
        || before.ends_with('/')
        || before.ends_with('%')
        || before.ends_with('&')
        || before.ends_with('|')
        || before.ends_with('^')
    {
        return false;
    }

    let first_word = before.split_whitespace().next().unwrap_or("");
    if matches!(first_word, "if" | "while" | "for" | "switch" | "catch") {
        return false;
    }

    true
}

pub(crate) fn collect_workspace_sources(root: &Path, lang: Language) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with('.')
                || name_str == "target"
                || name_str == "node_modules"
                || name_str == "build"
                || name_str == ".build"
                || name_str == "dist"
            {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() && is_candidate_source_file(&path, lang) {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

#[allow(clippy::too_many_arguments)]
pub async fn change_with(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    request: &[Param],
    modifiers: &Modifiers,
    apply: bool,
    force: bool,
) -> Result<SignatureChange> {
    let lang = Language::of(file).context("unsupported language for polyglot change_signature")?;
    let text = std::fs::read_to_string(file)
        .with_context(|| format!("cannot read file {}", file.display()))?;

    let decl = find_polyglot_declaration(&text, lang, line, col)?;
    let old_signature = text[decl.open_paren + 1..decl.close_paren].trim().to_string();
    let sources = collect_workspace_sources(root, lang);
    let has_call_candidates = sources.iter().any(|src_path| {
        let content = if src_path == file {
            text.clone()
        } else if let Ok(content) = std::fs::read_to_string(src_path) {
            content
        } else {
            return false;
        };
        content.match_indices(&decl.fn_name).any(|(at, _)| {
            let after = &content[at + decl.fn_name.len()..];
            !(at > 0 && content[..at].chars().next_back().is_some_and(is_ident))
                && !after.starts_with(is_ident)
                && !is_in_comment(&content, at, lang)
                && !crate::inline_parameter::is_in_string(&content, at, lang)
                && !is_import_or_export_context(&content, at, lang)
                && !(src_path == file
                    && at >= decl.open_paren.saturating_sub(decl.fn_name.len() + 20)
                    && at <= decl.close_paren)
                && call_args_span(&content, at + decl.fn_name.len()).is_some()
        })
    });
    let mut semantic_references = if has_call_candidates {
        crate::signature::references(remote, root, file, line, col)
            .await?
            .into_iter()
            .map(|(path, ref_line, ref_col)| {
                let path = std::fs::canonicalize(&path).unwrap_or(path);
                (path, ref_line, ref_col)
            })
            .collect::<HashSet<_>>()
    } else {
        HashSet::new()
    };

    // Safety check 1: verify all Keep params exist in declaration
    for r in request {
        if let Param::Keep(name) = r
            && !decl.params.iter().any(|d| &d.name == name)
        {
            anyhow::bail!("`{name}` is not a declared parameter");
        }
    }

    // Safety check 2: duplicate parameter names in request
    let mut seen = HashSet::new();
    for r in request {
        let name = match r {
            Param::Keep(n) => n,
            Param::Add { name, .. } => name,
        };
        if !seen.insert(name) {
            anyhow::bail!("duplicate parameter `{name}`");
        }
    }

    // Safety check 3: dropped parameters must not be used in the body unless force
    let mut dropped = Vec::new();
    for d in &decl.params {
        if !request.iter().any(|r| match r {
            Param::Keep(name) => name == &d.name,
            Param::Add { name, .. } => name == &d.name,
        }) {
            dropped.push(d.name.clone());
        }
    }

    if !dropped.is_empty() && !force {
        let body_text = &text[decl.body_open..decl.body_close];
        for gone in &dropped {
            if is_word_used(body_text, gone) {
                anyhow::bail!(
                    "parameter `{gone}` is still used in the function body; pass force: true to override"
                );
            }
        }
    }

    // Build the new declaration parameter list
    let mut new_decl_params = Vec::new();
    if let Some(r) = &decl.receiver {
        new_decl_params.push(r.clone());
    }

    for r in request {
        match r {
            Param::Keep(name) => {
                let orig = decl.params.iter().find(|d| &d.name == name).unwrap();
                new_decl_params.push(orig.raw.trim().to_string());
            }
            Param::Add { name, ty, value } => {
                new_decl_params.push(format_polyglot_param(name, ty, value, lang));
            }
        }
    }

    let new_signature = new_decl_params.join(", ");

    // Prepare declaration file edits
    let mut decl_edits: Vec<(usize, usize, String)> = Vec::new();

    // 1. Parameter list replacement
    decl_edits.push((
        decl.open_paren + 1,
        decl.close_paren,
        if new_signature.is_empty() {
            String::new()
        } else {
            new_signature.clone()
        },
    ));

    // 2. Return type modifier
    if let Some(new_ret) = &modifiers.returns {
        match lang {
            Language::TypeScript | Language::JavaScript => {
                if let Some((start, end)) = decl.ret_span {
                    decl_edits.push((start, end, format!(": {new_ret} ")));
                } else {
                    decl_edits.push((decl.close_paren + 1, decl.close_paren + 1, format!(": {new_ret} ")));
                }
            }
            Language::Python => {
                if let Some((start, end)) = decl.ret_span {
                    decl_edits.push((start, end, format!(" -> {new_ret}")));
                } else {
                    decl_edits.push((decl.close_paren + 1, decl.close_paren + 1, format!(" -> {new_ret}")));
                }
            }
            Language::Swift => {
                if let Some((start, end)) = decl.ret_span {
                    decl_edits.push((start, end, format!(" -> {new_ret} ")));
                } else {
                    decl_edits.push((decl.close_paren + 1, decl.close_paren + 1, format!(" -> {new_ret} ")));
                }
            }
            Language::Go => {
                if let Some((start, end)) = decl.ret_span {
                    decl_edits.push((start, end, format!(" {new_ret} ")));
                } else {
                    decl_edits.push((decl.close_paren + 1, decl.close_paren + 1, format!(" {new_ret} ")));
                }
            }
            Language::Cpp | Language::C | Language::Java => {
                if let Some((start, end)) = decl.ret_span {
                    let original = &text[start..end];
                    let indent = original.chars().take_while(|c| c.is_whitespace()).collect::<String>();
                    decl_edits.push((start, end, format!("{indent}{new_ret} ")));
                }
            }
            _ => {}
        }
    }

    // 3. Async modifier
    if let Some(want_async) = modifiers.asyncness {
        if want_async && !decl.is_async {
            match lang {
                Language::TypeScript | Language::JavaScript => {
                    let line_start = text[..decl.open_paren].rfind('\n').map_or(0, |p| p + 1);
                    if let Some(pos) = text[line_start..decl.open_paren].find("function") {
                        decl_edits.push((line_start + pos, line_start + pos, "async ".to_string()));
                    } else {
                        let indent_len = text[line_start..decl.open_paren]
                            .chars()
                            .take_while(|c| c.is_whitespace())
                            .count();
                        decl_edits.push((line_start + indent_len, line_start + indent_len, "async ".to_string()));
                    }
                }
                Language::Python => {
                    let line_start = text[..decl.open_paren].rfind('\n').map_or(0, |p| p + 1);
                    if let Some(pos) = text[line_start..decl.open_paren].find("def ") {
                        decl_edits.push((line_start + pos, line_start + pos, "async ".to_string()));
                    }
                }
                Language::Swift => {
                    decl_edits.push((decl.close_paren + 1, decl.close_paren + 1, " async".to_string()));
                }
                _ => {}
            }
        } else if !want_async && decl.is_async {
            match lang {
                Language::TypeScript | Language::JavaScript => {
                    if let Some((start, end)) = decl.async_keyword_span {
                        decl_edits.push((start, end, String::new()));
                    }
                }
                Language::Python => {
                    let line_start = text[..decl.open_paren].rfind('\n').map_or(0, |p| p + 1);
                    if let Some(pos) = text[line_start..decl.open_paren].find("async def ") {
                        decl_edits.push((line_start + pos, line_start + pos + 6, String::new()));
                    }
                }
                Language::Swift => {
                    if let Some(idx) = text[decl.close_paren..decl.body_open].find("async") {
                        let start = decl.close_paren + idx;
                        decl_edits.push((start, start + 5, String::new()));
                    }
                }
                _ => {}
            }
        }
    }

    // 4. Visibility modifier
    if let Some(vis) = &modifiers.visibility {
        match lang {
            Language::TypeScript | Language::JavaScript => {
                if let Some((start, end)) = decl.visibility_span {
                    decl_edits.push((start, end, format!("{vis} ")));
                } else {
                    let line_start = text[..decl.open_paren].rfind('\n').map_or(0, |p| p + 1);
                    let indent_len = text[line_start..decl.open_paren]
                        .chars()
                        .take_while(|c| c.is_whitespace())
                        .count();
                    decl_edits.push((line_start + indent_len, line_start + indent_len, format!("{vis} ")));
                }
            }
            Language::Swift => {
                if let Some((start, end)) = decl.visibility_span {
                    decl_edits.push((start, end, format!("{vis} ")));
                } else {
                    let line_start = text[..decl.open_paren].rfind('\n').map_or(0, |p| p + 1);
                    if let Some(pos) = text[line_start..decl.open_paren].find("func ") {
                        let indent_len = text[line_start..line_start + pos]
                            .chars()
                            .take_while(|c| c.is_whitespace())
                            .count();
                        decl_edits.push((line_start + indent_len, line_start + pos, format!("{vis} ")));
                    }
                }
            }
            _ => {}
        }
    }

    // Apply call site edits across the workspace
    let mut rewritten_files = Vec::new();
    let mut unmatched = Vec::new();
    for src_path in &sources {
        let is_decl_file = src_path == file;
        let content = if is_decl_file {
            text.clone()
        } else {
            let Ok(c) = std::fs::read_to_string(src_path) else {
                continue;
            };
            c
        };

        let mut file_edits: Vec<(usize, usize, String)> = if is_decl_file {
            decl_edits.clone()
        } else {
            Vec::new()
        };

        // Scan for calls to `decl.fn_name`
        let fn_name = &decl.fn_name;
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

            let source_path = std::fs::canonicalize(src_path).unwrap_or_else(|_| src_path.clone());
            let (line_num, col_num) = one_based_lsp_position(&content, at);
            let reference_key = (source_path, line_num, col_num);
            if is_import_or_export_context(&content, at, lang) {
                semantic_references.remove(&reference_key);
                continue;
            }
            // Skip comments and strings.
            if is_in_comment(&content, at, lang)
                || crate::inline_parameter::is_in_string(&content, at, lang)
            {
                continue;
            }

            // Skip declaration itself in declaration file
            if is_decl_file && at >= decl.open_paren.saturating_sub(fn_name.len() + 20) && at <= decl.close_paren {
                continue;
            }

            // Check if call args follow
            let Some((args_start, args_end)) = call_args_span(&content, at + fn_name.len()) else {
                continue;
            };
            let site = format!("{}:{line_num}:{col_num}", display(root, src_path));

            // Check if C/C++ prototype
            if matches!(lang, Language::Cpp | Language::C)
                && is_c_cpp_prototype(&content, at, args_start, args_end)
            {
                if content[args_start..args_end].trim() != old_signature.trim() {
                    continue;
                }
                let after_paren = content[args_end + 1..].trim_start();
                if after_paren.starts_with(';') {
                    // It's a prototype: rewrite prototype parameter list and return type
                    file_edits.push((args_start, args_end, new_signature.clone()));
                    if let Some(new_ret) = &modifiers.returns {
                        let line_start = content[..at].rfind('\n').map_or(0, |p| p + 1);
                        let before_fn = &content[line_start..at];
                        let indent = before_fn.chars().take_while(|c| c.is_whitespace()).collect::<String>();
                        file_edits.push((line_start, at, format!("{indent}{new_ret} ")));
                    }
                    continue;
                }
            }

            if !semantic_references.remove(&reference_key) {
                continue;
            }

            // Call site rewriting
            let old_args_str = &content[args_start..args_end];
            let old_args = split_args(old_args_str);
            let is_python_keyword_call = lang == Language::Python
                && old_args.iter().any(|a| a.split_once('=').is_some());
            let kept_indices = request
                .iter()
                .filter_map(|r| match r {
                    Param::Keep(name) => decl.params.iter().position(|d| &d.name == name),
                    Param::Add { .. } => None,
                })
                .collect::<Vec<_>>();
            let reordered = kept_indices.windows(2).any(|pair| pair[0] > pair[1]);
            let dropped = kept_indices.len() < decl.params.len();
            let effectful_existing = old_args.iter().any(|arg| !is_side_effect_free_argument(arg, lang));
            let effectful_added = request.iter().any(|r| {
                matches!(r, Param::Add { value, .. } if !is_side_effect_free_argument(value, lang))
            });
            if ((reordered || dropped) && effectful_existing) || effectful_added {
                unmatched.push(format!(
                    "{site} has argument side effects that cannot safely survive this signature change"
                ));
                continue;
            }

            let mut new_args = Vec::new();
            for r in request {
                match r {
                    Param::Keep(name) => {
                        let orig_idx = decl.params.iter().position(|d| &d.name == name);
                        let orig_param = decl.params.iter().find(|d| &d.name == name);

                        // Look up argument in old_args
                        let mut arg_val = None;
                        if lang == Language::Python {
                            // First check keyword arguments
                            for a in &old_args {
                                let a_trimmed = a.trim();
                                if let Some((k, _v)) = a_trimmed.split_once('=')
                                    && k.trim() == name
                                {
                                    arg_val = Some(a_trimmed.to_string());
                                    break;
                                }
                            }
                            if arg_val.is_none() && let Some(idx) = orig_idx {
                                let pos = if decl.receiver.is_some() && !content[..at].trim_end().ends_with('.') {
                                    idx + 1
                                } else {
                                    idx
                                };
                                if let Some(a) = old_args.get(pos) {
                                    let a_trimmed = a.trim();
                                    if is_python_keyword_call {
                                        arg_val = Some(format!("{name}={a_trimmed}"));
                                    } else {
                                        arg_val = Some(a_trimmed.to_string());
                                    }
                                }
                            }
                        } else if lang == Language::Swift {
                            if let Some(orig_p) = orig_param {
                                for a in &old_args {
                                    let a_trimmed = a.trim();
                                    if let Some((lbl, val)) = a_trimmed.split_once(':')
                                        && (lbl.trim() == name || orig_p.label.as_deref() == Some(lbl.trim()))
                                    {
                                        arg_val = Some(format!("{}: {}", lbl.trim(), val.trim()));
                                        break;
                                    }
                                }
                                if arg_val.is_none()
                                    && let Some(idx) = orig_idx
                                    && let Some(a) = old_args.get(idx)
                                {
                                    let a_trimmed = a.trim();
                                    if let Some(lbl) = &orig_p.label {
                                        if lbl != "_" {
                                            arg_val = Some(format!("{lbl}: {a_trimmed}"));
                                        } else {
                                            arg_val = Some(a_trimmed.to_string());
                                        }
                                    } else {
                                        arg_val = Some(format!("{name}: {a_trimmed}"));
                                    }
                                }
                            }
                        } else if let Some(idx) = orig_idx
                            && let Some(a) = old_args.get(idx)
                        {
                            arg_val = Some(a.trim().to_string());
                        }

                        if let Some(val) = arg_val {
                            new_args.push(val);
                        } else if let Some(orig_p) = orig_param && let Some(def) = &orig_p.default {
                            new_args.push(def.clone());
                        }
                    }
                    Param::Add { name, ty: _, value } => {
                        if lang == Language::Swift {
                            new_args.push(format!("{name}: {value}"));
                        } else if is_python_keyword_call {
                            new_args.push(format!("{name}={value}"));
                        } else {
                            new_args.push(value.clone());
                        }
                    }
                }
            }

            let new_call_args = new_args.join(", ");
            file_edits.push((args_start, args_end, new_call_args));

            // Check if call needs `await`
            if modifiers.asyncness == Some(true) {
                let before_call = content[..at].trim_end();
                if !before_call.ends_with("await") {
                    let after_call = content[args_end + 1..].trim_start();
                    if after_call.starts_with('.') || after_call.starts_with('[') {
                        // Parenthesize
                        file_edits.push((at, at, "(await ".to_string()));
                        file_edits.push((args_end + 1, args_end + 1, ")".to_string()));
                    } else {
                        file_edits.push((at, at, "await ".to_string()));
                    }
                }
            } else if modifiers.asyncness == Some(false) {
                let before_call = content[..at].trim_end();
                if let Some(await_start) = before_call.strip_suffix("await").map(str::len)
                    && !content[..await_start].chars().next_back().is_some_and(is_ident)
                {
                    file_edits.push((await_start, at, String::new()));
                }
            }
        }

        if !file_edits.is_empty() {
            // Sort edits in descending order of start offset to avoid shifting
            file_edits.sort_by_key(|e| std::cmp::Reverse(e.0));
            let mut new_content = content;
            for (start, end, replacement) in file_edits {
                if start <= end && end <= new_content.len() {
                    new_content.replace_range(start..end, &replacement);
                }
            }
            let rel_path = display(root, src_path);
            rewritten_files.push((rel_path, new_content));
        }
    }

    for (path, ref_line, ref_col) in semantic_references {
        unmatched.push(format!(
            "{}:{ref_line}:{ref_col}: analyzer reference to `{}` could not be rewritten safely",
            display(root, &path),
            decl.fn_name
        ));
    }

    // Format rule description
    let rule = format!("{}({old_signature}) -> {}({new_signature})", decl.fn_name, decl.fn_name);

    // In-memory overlay validation
    let files_to_validate: Vec<(PathBuf, String)> = rewritten_files
        .iter()
        .map(|(p, t)| (PathBuf::from(p), t.clone()))
        .collect();
    let reports = crate::diagnostics::validate_texts(remote, root, &files_to_validate, &[]).await?;
    let overlay_diags: Vec<String> = reports
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

    // If apply is true and no diagnostics, write all files
    if apply {
        anyhow::ensure!(
            unmatched.is_empty(),
            "{} call site(s) could not be rewritten safely; nothing was written:\n  {}",
            unmatched.len(),
            unmatched.join("\n  ")
        );
        anyhow::ensure!(
            overlay_diags.is_empty() || force,
            "the analyzer reports {} error(s); nothing was written:\n  {}",
            overlay_diags.len(),
            overlay_diags.join("\n  ")
        );
        for (rel, content) in &rewritten_files {
            let full_path = root.join(rel);
            std::fs::write(&full_path, content)
                .with_context(|| format!("cannot write {}", full_path.display()))?;
        }
    }

    Ok(SignatureChange {
        symbol: decl.fn_name,
        root: root.to_path_buf(),
        file: display(root, file),
        old_signature,
        new_signature,
        rule,
        rewritten: rewritten_files,
        unmatched,
        unexpected: Vec::new(),
        diagnostics: overlay_diags,
        applied: apply,
        returns: modifiers.returns.as_ref().map(|r| (String::new(), r.clone())),
        visibility: modifiers.visibility.as_ref().map(|v| (String::new(), v.clone())),
        asyncness: modifiers.asyncness.map(|a| (!a, a)),
        not_async: Vec::new(),
    })
}
