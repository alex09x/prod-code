//! Polyglot refactor.move across TypeScript/JavaScript, Python, Go, C++, and Swift (Roadmap 7.1.1, Epic #516).
//!
//! Relocates top-level declarations (functions, classes, interfaces, types, structs, enums, constants)
//! into another module, updating declarations, carrying dependencies, and rewriting caller imports
//! across the workspace with full diagnostic pre-validation.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use crate::move_item::Move;
use crate::parameter_object::Language;

fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn display_relative_or_name(from_file: &Path, to_file: &Path) -> String {
    let from_dir = from_file.parent().unwrap_or(Path::new(""));
    let rel = path_relative_from(to_file, from_dir);
    let s = rel.to_string_lossy().replace('\\', "/");
    if s.is_empty() {
        to_file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    } else {
        s
    }
}

fn module_display_name(file: &Path, root: &Path, lang: Language) -> String {
    match lang {
        Language::Python => python_module_specifier(file, file, root),
        Language::Go => get_go_package(file),
        _ => display(root, file),
    }
}

pub fn is_compatible_language_family(a: Language, b: Language) -> bool {
    if a == b {
        return true;
    }
    matches!(
        (a, b),
        (Language::TypeScript, Language::JavaScript)
            | (Language::JavaScript, Language::TypeScript)
            | (Language::Cpp, Language::C)
            | (Language::C, Language::Cpp)
    )
}

pub fn path_relative_from(path: &Path, base: &Path) -> PathBuf {
    let path_comps: Vec<_> = path.components().collect();
    let base_comps: Vec<_> = base.components().collect();

    let mut common = 0;
    while common < path_comps.len()
        && common < base_comps.len()
        && path_comps[common] == base_comps[common]
    {
        common += 1;
    }

    let mut result = PathBuf::new();
    for _ in common..base_comps.len() {
        result.push("..");
    }
    for comp in &path_comps[common..] {
        result.push(comp.as_os_str());
    }
    if result.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        result
    }
}

pub fn relative_import_specifier(from_file: &Path, to_file: &Path) -> String {
    let from_dir = from_file.parent().unwrap_or(Path::new(""));
    let mut to_str = to_file.to_string_lossy().into_owned();
    for ext in &[".d.ts", ".tsx", ".ts", ".jsx", ".js"] {
        if let Some(stripped) = to_str.strip_suffix(ext) {
            to_str = stripped.to_string();
            break;
        }
    }
    let to_path = PathBuf::from(&to_str);
    let rel = path_relative_from(&to_path, from_dir);
    let mut s = rel.to_string_lossy().replace('\\', "/");
    if !s.starts_with("./") && !s.starts_with("../") {
        s = format!("./{s}");
    }
    s
}

pub fn python_module_specifier(from_file: &Path, to_file: &Path, root: &Path) -> String {
    let _ = from_file;
    let rel = to_file.strip_prefix(root).unwrap_or(to_file);
    let mut s = rel.to_string_lossy().into_owned();
    if let Some(stripped) = s.strip_suffix(".py") {
        s = stripped.to_string();
    }
    if let Some(stripped) = s.strip_suffix("/__init__") {
        s = stripped.to_string();
    }
    s.replace(['/', '\\'], ".")
}

pub fn is_symbol_used(content: &str, symbol: &str) -> bool {
    for (at, _) in content.match_indices(symbol) {
        if at > 0 {
            let prev = content[..at].chars().next_back().unwrap();
            if prev.is_alphanumeric() || prev == '_' {
                continue;
            }
        }
        let after = &content[at + symbol.len()..];
        if after.starts_with(|c: char| c.is_alphanumeric() || c == '_') {
            continue;
        }
        let line_start = content[..at].rfind('\n').map_or(0, |p| p + 1);
        let before_on_line = content[line_start..at].trim_start();
        if before_on_line.starts_with("//")
            || before_on_line.starts_with('#')
            || before_on_line.starts_with('*')
        {
            continue;
        }
        return true;
    }
    false
}

pub fn with_doc_comment_polyglot(text: &str, start: u32, lang: Language) -> u32 {
    let lines: Vec<&str> = text.lines().collect();
    let mut first = start;
    while first > 1 {
        let above = lines.get(first as usize - 2).map(|l| l.trim()).unwrap_or("");
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

fn is_polyglot_decl_header(line: &str, lang: Language) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    match lang {
        Language::Python => {
            let indent = line.len() - line.trim_start().len();
            indent == 0
                && (trimmed.starts_with("def ")
                    || trimmed.starts_with("async def ")
                    || trimmed.starts_with("class "))
        }
        Language::TypeScript | Language::JavaScript => {
            if trimmed.starts_with("import ")
                || trimmed.starts_with("export {")
                || trimmed.starts_with("export *")
            {
                return false;
            }
            trimmed.starts_with("export default ")
                || trimmed.starts_with("export function ")
                || trimmed.starts_with("export async function ")
                || trimmed.starts_with("function ")
                || trimmed.starts_with("async function ")
                || trimmed.starts_with("export class ")
                || trimmed.starts_with("class ")
                || trimmed.starts_with("export interface ")
                || trimmed.starts_with("interface ")
                || trimmed.starts_with("export type ")
                || trimmed.starts_with("type ")
                || trimmed.starts_with("export enum ")
                || trimmed.starts_with("enum ")
                || trimmed.starts_with("export const ")
                || trimmed.starts_with("const ")
                || trimmed.starts_with("export let ")
                || trimmed.starts_with("let ")
                || trimmed.starts_with("export var ")
                || trimmed.starts_with("var ")
        }
        Language::Go => {
            let indent = line.len() - line.trim_start().len();
            indent == 0
                && (trimmed.starts_with("func ")
                    || trimmed.starts_with("type ")
                    || trimmed.starts_with("var ")
                    || trimmed.starts_with("const "))
        }
        Language::Swift => {
            let rest = trimmed
                .strip_prefix("public ")
                .or_else(|| trimmed.strip_prefix("open "))
                .or_else(|| trimmed.strip_prefix("internal "))
                .or_else(|| trimmed.strip_prefix("fileprivate "))
                .or_else(|| trimmed.strip_prefix("private "))
                .unwrap_or(trimmed);
            rest.starts_with("func ")
                || rest.starts_with("class ")
                || rest.starts_with("struct ")
                || rest.starts_with("enum ")
                || rest.starts_with("protocol ")
        }
        Language::Cpp | Language::C => {
            let indent = line.len() - line.trim_start().len();
            indent == 0
                && !trimmed.starts_with('#')
                && !trimmed.starts_with("using ")
                && (trimmed.starts_with("class ")
                    || trimmed.starts_with("struct ")
                    || trimmed.starts_with("enum ")
                    || (trimmed.contains('(') && (trimmed.contains(')') || trimmed.ends_with('{'))))
        }
        Language::Rust => false,
        Language::Java => {
            trimmed.starts_with("public ")
                || trimmed.starts_with("protected ")
                || trimmed.starts_with("private ")
                || trimmed.starts_with("class ")
                || trimmed.starts_with("interface ")
                || trimmed.starts_with("record ")
                || trimmed.starts_with("enum ")
        }
    }
}

fn extract_polyglot_decl_name(line: &str, lang: Language) -> Option<String> {
    let trimmed = line.trim();
    match lang {
        Language::Python => {
            if let Some(pos) = trimmed.find("def ") {
                let after = &trimmed[pos + 4..];
                let paren = after.find('(').unwrap_or(after.len());
                let name = after[..paren].trim();
                return Some(name.to_string());
            }
            if let Some(pos) = trimmed.find("class ") {
                let after = &trimmed[pos + 6..];
                let paren = after.find('(').or_else(|| after.find(':')).unwrap_or(after.len());
                let name = after[..paren].trim();
                return Some(name.to_string());
            }
        }
        Language::TypeScript | Language::JavaScript => {
            let rest = trimmed
                .strip_prefix("export default ")
                .or_else(|| trimmed.strip_prefix("export "))
                .unwrap_or(trimmed);
            let rest = rest.strip_prefix("async ").unwrap_or(rest);
            for keyword in &[
                "function ",
                "class ",
                "interface ",
                "type ",
                "enum ",
                "const ",
                "let ",
                "var ",
            ] {
                if let Some(after) = rest.strip_prefix(keyword) {
                    let clean = after.trim_start();
                    let name = clean
                        .split(|c: char| !c.is_alphanumeric() && c != '_')
                        .next()
                        .unwrap_or("");
                    if !name.is_empty() {
                        return Some(name.to_string());
                    }
                }
            }
        }
        Language::Go => {
            if let Some(after) = trimmed.strip_prefix("func ") {
                let clean = after.trim_start();
                if clean.starts_with('(')
                    && let Some(close_recv) = clean.find(')')
                {
                    let after_recv = clean[close_recv + 1..].trim_start();
                    let name = after_recv
                        .split(|c: char| !c.is_alphanumeric() && c != '_')
                        .next()
                        .unwrap_or("");
                    return Some(name.to_string());
                }
                let name = clean
                    .split(|c: char| !c.is_alphanumeric() && c != '_')
                    .next()
                    .unwrap_or("");
                if !name.is_empty() {
                    return Some(name.to_string());
                }
            }
            if let Some(after) = trimmed.strip_prefix("type ") {
                let name = after.split_whitespace().next().unwrap_or("");
                if !name.is_empty() {
                    return Some(name.to_string());
                }
            }
            if let Some(after) = trimmed
                .strip_prefix("const ")
                .or_else(|| trimmed.strip_prefix("var "))
            {
                let name = after
                    .trim_start()
                    .split(|c: char| !c.is_alphanumeric() && c != '_')
                    .next()
                    .unwrap_or("");
                if !name.is_empty() {
                    return Some(name.to_string());
                }
            }
        }
        Language::Swift => {
            let rest = trimmed
                .strip_prefix("public ")
                .or_else(|| trimmed.strip_prefix("open "))
                .or_else(|| trimmed.strip_prefix("internal "))
                .or_else(|| trimmed.strip_prefix("fileprivate "))
                .or_else(|| trimmed.strip_prefix("private "))
                .unwrap_or(trimmed);
            for keyword in &["func ", "class ", "struct ", "enum ", "protocol "] {
                if let Some(after) = rest.strip_prefix(keyword) {
                    let clean = after.trim_start();
                    let name = clean
                        .split(|c: char| !c.is_alphanumeric() && c != '_')
                        .next()
                        .unwrap_or("");
                    if !name.is_empty() {
                        return Some(name.to_string());
                    }
                }
            }
        }
        Language::Cpp | Language::C => {
            if let Some(paren) = trimmed.find('(') {
                let before = trimmed[..paren].trim();
                if let Some(name) = before.split_whitespace().last() {
                    let clean = name.trim_start_matches('*').trim_start_matches('&');
                    let member = clean.rsplit("::").next().unwrap_or(clean);
                    if !member.is_empty()
                        && !matches!(member, "if" | "while" | "for" | "switch" | "catch")
                    {
                        return Some(member.to_string());
                    }
                }
            }
            for keyword in &["class ", "struct ", "enum "] {
                if let Some(after) = trimmed.strip_prefix(keyword) {
                    let clean = after.trim_start();
                    let name = clean
                        .split(|c: char| !c.is_alphanumeric() && c != '_')
                        .next()
                        .unwrap_or("");
                    if !name.is_empty() {
                        return Some(name.to_string());
                    }
                }
            }
        }
        Language::Rust => {}
        Language::Java => {
            if let Some(paren) = trimmed.find('(') {
                let before = trimmed[..paren].trim();
                if let Some(name) = before.split_whitespace().last() {
                    let clean = name.trim();
                    if !clean.is_empty()
                        && !matches!(clean, "if" | "while" | "for" | "switch" | "catch")
                    {
                        return Some(clean.to_string());
                    }
                }
            }
            for keyword in &["class ", "interface ", "record ", "enum "] {
                if let Some(pos) = trimmed.find(keyword) {
                    let after = &trimmed[pos + keyword.len()..];
                    let clean = after.trim_start();
                    let name = clean
                        .split(|c: char| !c.is_alphanumeric() && c != '_')
                        .next()
                        .unwrap_or("");
                    if !name.is_empty() {
                        return Some(name.to_string());
                    }
                }
            }
        }
    }
    None
}

pub fn find_polyglot_decl(
    text: &str,
    line: u32,
    lang: Language,
) -> Result<(String, u32, u32)> {
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
        for (i, line_str) in lines.iter().enumerate().take(target_idx + 20).skip(target_idx + 1) {
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
        anyhow::bail!(
            "`{name}` is a method with a receiver; move it with `code_move_method`"
        );
    }

    let decl_name = extract_polyglot_decl_name(lines[header_idx], lang)
        .with_context(|| format!("could not parse declaration name at line {}", header_idx + 1))?;

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

pub fn format_item_for_target(item: &str, lang: Language) -> String {
    if !matches!(lang, Language::TypeScript | Language::JavaScript) {
        return item.to_string();
    }
    let lines: Vec<&str> = item.lines().collect();
    let mut out = Vec::new();
    let mut exported = false;

    for line in lines {
        let trimmed = line.trim();
        if !exported
            && !trimmed.is_empty()
            && !trimmed.starts_with("//")
            && !trimmed.starts_with("/*")
            && !trimmed.starts_with('*')
            && !trimmed.starts_with('@')
        {
            if trimmed.starts_with("export ") {
                exported = true;
                out.push(line.to_string());
            } else if trimmed.starts_with("function ")
                || trimmed.starts_with("async function ")
                || trimmed.starts_with("class ")
                || trimmed.starts_with("interface ")
                || trimmed.starts_with("type ")
                || trimmed.starts_with("enum ")
                || trimmed.starts_with("const ")
                || trimmed.starts_with("let ")
                || trimmed.starts_with("var ")
            {
                let indent = &line[..line.len() - line.trim_start().len()];
                out.push(format!("{indent}export {}", line.trim_start()));
                exported = true;
            } else {
                out.push(line.to_string());
            }
        } else {
            out.push(line.to_string());
        }
    }
    out.join("
")
}

pub fn check_target_collision(target_text: &str, name: &str, lang: Language) -> Result<()> {
    if target_text.trim().is_empty() {
        return Ok(());
    }
    for (line_num, line) in target_text.lines().enumerate() {
        let trimmed = line.trim();
        let is_decl = match lang {
            Language::TypeScript | Language::JavaScript => {
                trimmed.starts_with("export function ")
                    || trimmed.starts_with("function ")
                    || trimmed.starts_with("export class ")
                    || trimmed.starts_with("class ")
                    || trimmed.starts_with("export interface ")
                    || trimmed.starts_with("interface ")
                    || trimmed.starts_with("export type ")
                    || trimmed.starts_with("type ")
                    || trimmed.starts_with("export const ")
                    || trimmed.starts_with("const ")
                    || trimmed.starts_with("export enum ")
                    || trimmed.starts_with("enum ")
            }
            Language::Python => {
                line.len() - line.trim_start().len() == 0
                    && (trimmed.starts_with("def ")
                        || trimmed.starts_with("async def ")
                        || trimmed.starts_with("class "))
            }
            Language::Go => {
                trimmed.starts_with("func ")
                    || trimmed.starts_with("type ")
                    || trimmed.starts_with("var ")
                    || trimmed.starts_with("const ")
            }
            Language::Swift => {
                trimmed.contains("func ")
                    || trimmed.contains("class ")
                    || trimmed.contains("struct ")
                    || trimmed.contains("enum ")
                    || trimmed.contains("protocol ")
            }
            Language::Cpp | Language::C | Language::Java => {
                trimmed.contains("class ") || trimmed.contains("interface ") || trimmed.contains("record ") || trimmed.contains("enum ") || trimmed.contains(name)
            }
            Language::Rust => false,
        };

        if is_decl && is_symbol_used(line, name) {
            anyhow::bail!("target already declares `{name}` at line {}", line_num + 1);
        }
    }
    Ok(())
}

fn top_import_insertion_pos(content: &str) -> usize {
    let mut pos = 0;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("#!")
            || trimmed.starts_with("//")
            || trimmed.starts_with("/*")
            || trimmed.starts_with('*')
            || trimmed == "\"use strict\";"
            || trimmed == "'use strict';"
        {
            pos += line.len() + 1;
            continue;
        }
        break;
    }
    pos.min(content.len())
}

pub(crate) fn insert_or_merge_ts_import(content: &str, sym: &str, rel_path: &str) -> (String, bool) {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("import ") && trimmed.contains(rel_path) {
            if is_symbol_used(line, sym) {
                return (content.to_string(), false);
            }
            if let Some(open) = line.find('{')
                && let Some(close) = line.find('}')
            {
                let inside = line[open + 1..close].trim();
                let new_inside = if inside.is_empty() {
                    sym.to_string()
                } else {
                    format!("{inside}, {sym}")
                };
                let new_line = format!("{}{new_inside}{}", &line[..open + 1], &line[close..]);
                let replaced = content.replace(line, &new_line);
                return (replaced, true);
            }
        }
    }
    let import_line = format!("import {{ {sym} }} from \"{rel_path}\";\n");
    let insert_pos = top_import_insertion_pos(content);
    let mut out = String::with_capacity(content.len() + import_line.len());
    out.push_str(&content[..insert_pos]);
    out.push_str(&import_line);
    out.push_str(&content[insert_pos..]);
    (out, true)
}

pub(crate) fn insert_or_merge_py_import(content: &str, sym: &str, mod_spec: &str) -> (String, bool) {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("from ") && trimmed.contains(mod_spec) && trimmed.contains(" import ") {
            if is_symbol_used(line, sym) {
                return (content.to_string(), false);
            }
            let new_line = format!("{line}, {sym}");
            let replaced = content.replace(line, &new_line);
            return (replaced, true);
        }
    }
    let import_line = format!("from {mod_spec} import {sym}\n");
    let insert_pos = top_import_insertion_pos(content);
    let mut out = String::with_capacity(content.len() + import_line.len());
    out.push_str(&content[..insert_pos]);
    out.push_str(&import_line);
    out.push_str(&content[insert_pos..]);
    (out, true)
}

fn get_go_package(file: &Path) -> String {
    if let Ok(content) = std::fs::read_to_string(file) {
        for line in content.lines() {
            let t = line.trim();
            if let Some(rest) = t.strip_prefix("package ") {
                let name = rest.trim();
                if !name.is_empty() {
                    return name.to_string();
                }
            }
        }
    }
    file.parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "main".to_string())
}

fn go_module_import_path(target_dir: &Path) -> Result<String> {
    let target = std::fs::canonicalize(target_dir).unwrap_or_else(|_| target_dir.to_path_buf());
    let mut directory = Some(target.as_path());
    while let Some(dir) = directory {
        if let Ok(manifest) = std::fs::read_to_string(dir.join("go.mod"))
            && let Some(module) = manifest.lines().find_map(|line| {
                line.trim()
                    .strip_prefix("module ")
                    .map(|value| value.trim().trim_matches('"').to_string())
                    .filter(|value| !value.is_empty())
            })
        {
            let relative = target.strip_prefix(dir).unwrap_or(Path::new(""));
            let suffix = relative.to_string_lossy().replace('\\', "/");
            return Ok(if suffix.is_empty() {
                module
            } else {
                format!("{module}/{suffix}")
            });
        }
        directory = dir.parent();
    }
    anyhow::bail!(
        "cannot find the Go module path for {}",
        target_dir.display()
    )
}

fn rewrite_go_call_sites(
    source_text: &str,
    decl_name: &str,
    target_pkg: &str,
) -> Result<(String, usize)> {
    let mut edits = Vec::new();
    for (at, _) in source_text.match_indices(decl_name) {
        if (at > 0 && source_text[..at].chars().next_back().is_some_and(is_ident))
            || source_text[at + decl_name.len()..].chars().next().is_some_and(is_ident)
            || crate::inline_parameter::is_in_comment(source_text, at, Language::Go)
            || crate::inline_parameter::is_in_string(source_text, at, Language::Go)
        {
            continue;
        }
        let before = source_text[..at].trim_end();
        if before.ends_with('.') {
            continue; // A selector, which may name an unrelated method.
        }
        let after = source_text[at + decl_name.len()..].trim_start();
        if after.starts_with('(') {
            let line_start = source_text[..at].rfind('\n').map_or(0, |i| i + 1);
            if source_text[line_start..at].trim_start().starts_with("func ") {
                continue; // A separate declaration is not a call site.
            }
            edits.push((at, decl_name.len()));
        } else {
            anyhow::bail!(
                "Go reference `{decl_name}` at byte {at} is not a call; cannot safely qualify it"
            );
        }
    }
    let count = edits.len();
    let mut rewritten = source_text.to_string();
    for (at, len) in edits.into_iter().rev() {
        rewritten.replace_range(at..at + len, &format!("{target_pkg}.{decl_name}"));
    }
    Ok((rewritten, count))
}

fn add_go_import(content: &str, pkg: &str) -> String {
    let import_str = format!("import \"{pkg}\"\n");
    if let Some(pos) = content.find("package ") {
        let after_pkg = content[pos..].find('\n').map_or(pos + 8, |p| pos + p + 1);
        let mut out = String::new();
        out.push_str(&content[..after_pkg]);
        out.push('\n');
        out.push_str(&import_str);
        out.push_str(&content[after_pkg..]);
        out
    } else {
        format!("{import_str}\n{content}")
    }
}

fn rewrite_go_cross_pkg_in_source(
    source_text: &str,
    decl_name: &str,
    target_pkg: &str,
    target_file: &Path,
    root: &Path,
) -> Result<(String, String)> {
    let target_dir = target_file.parent().unwrap_or(root);
    let target_import_path = go_module_import_path(target_dir)?;
    anyhow::ensure!(
        !source_text.contains(&format!("\"{target_import_path}\"")),
        "the target Go module is already imported and its local alias cannot be safely resolved"
    );
    let (replaced, calls) = rewrite_go_call_sites(source_text, decl_name, target_pkg)?;
    if calls == 0 {
        return Ok((source_text.to_string(), String::new()));
    }
    let with_import = add_go_import(&replaced, &target_import_path);
    Ok((with_import, format!("imported \"{target_import_path}\"")))
}

pub fn carry_imports_polyglot(
    source_text: &str,
    item_text: &str,
    target_text: &str,
    source_file: &Path,
    target_file: &Path,
    lang: Language,
    _root: &Path,
) -> (String, Vec<String>) {
    let mut out = target_text.to_string();
    let mut notes = Vec::new();

    match lang {
        Language::TypeScript | Language::JavaScript => {
            for line in source_text.lines() {
                let trimmed = line.trim();
                if !trimmed.starts_with("import ") {
                    continue;
                }
                let Some(from_idx) = trimmed.find(" from ") else {
                    continue;
                };
                let specifier = trimmed[from_idx + 6..].trim().trim_matches(['\x27', '"', ';'].as_slice());
                if let Some(open) = trimmed.find('{')
                    && let Some(close) = trimmed.find('}')
                {
                    let inside = &trimmed[open + 1..close];
                    for raw_sym in inside.split(',') {
                        let sym = raw_sym.split_whitespace().next().unwrap_or("");
                        if !sym.is_empty() && is_symbol_used(item_text, sym) && !is_symbol_used(&out, sym) {
                            let target_spec = if specifier.starts_with('.') {
                                let source_dir = source_file.parent().unwrap_or(Path::new(""));
                                let resolved = source_dir.join(specifier);
                                relative_import_specifier(target_file, &resolved)
                            } else {
                                specifier.to_string()
                            };
                            let (new_out, added) = insert_or_merge_ts_import(&out, sym, &target_spec);
                            if added {
                                out = new_out;
                                notes.push(format!("carried import `{sym}` from `{target_spec}`"));
                            }
                        }
                    }
                }
            }
        }
        Language::Python => {
            for line in source_text.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("from ") || trimmed.starts_with("import ") {
                    let words: Vec<&str> = trimmed.split_whitespace().collect();
                    for &w in &words {
                        let clean = w.trim_matches(['(', ')', ',', ':'].as_slice());
                        if !clean.is_empty()
                            && clean != "from"
                            && clean != "import"
                            && clean != "as"
                            && is_symbol_used(item_text, clean)
                            && !out.contains(clean)
                            && !out.contains(trimmed)
                        {
                            out = format!("{trimmed}\n{out}");
                            notes.push(format!("carried `{trimmed}`"));
                            break;
                        }
                    }
                }
            }
        }
        Language::Go => {
            let go_packages = [
                "fmt", "strings", "os", "io", "time", "errors", "sync", "bytes", "math",
                "path", "sort",
            ];
            for pkg in go_packages {
                let needle = format!("{pkg}.");
                if item_text.contains(&needle) && !out.contains(&format!("\"{pkg}\"")) {
                    out = add_go_import(&out, pkg);
                    notes.push(format!("carried import \"{pkg}\""));
                }
            }
        }
        Language::Swift => {
            for line in source_text.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("import ") {
                    let mod_name = trimmed.strip_prefix("import ").unwrap_or("").trim();
                    if !mod_name.is_empty() && !out.contains(trimmed) {
                        out = format!("{trimmed}\n{out}");
                        notes.push(format!("carried `{trimmed}`"));
                    }
                }
            }
        }
        Language::Cpp | Language::C => {
            for line in source_text.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("#include") && !out.contains(trimmed) {
                    out = format!("{trimmed}\n{out}");
                    notes.push(format!("carried `{trimmed}`"));
                }
            }
        }
        Language::Rust => {}
        Language::Java => {
            for line in source_text.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("import ") && !out.contains(trimmed) {
                    out = format!("{trimmed}\n{out}");
                    notes.push(format!("carried `{trimmed}`"));
                }
            }
        }
    }

    (out, notes)
}

pub fn update_source_imports(
    source_text: &str,
    decl_name: &str,
    source_file: &Path,
    target_file: &Path,
    root: &Path,
    lang: Language,
) -> Result<(String, Option<String>)> {
    if !is_symbol_used(source_text, decl_name) {
        return Ok((source_text.to_string(), None));
    }

    match lang {
        Language::TypeScript | Language::JavaScript => {
            let rel = relative_import_specifier(source_file, target_file);
            let (new_text, added) = insert_or_merge_ts_import(source_text, decl_name, &rel);
            let note = if added {
                Some(format!("imported `{decl_name}` from `{rel}`"))
            } else {
                None
            };
            Ok((new_text, note))
        }
        Language::Python => {
            let mod_spec = python_module_specifier(source_file, target_file, root);
            let (new_text, added) = insert_or_merge_py_import(source_text, decl_name, &mod_spec);
            let note = if added {
                Some(format!("from {mod_spec} import {decl_name}"))
            } else {
                None
            };
            Ok((new_text, note))
        }
        Language::Go => {
            let src_pkg = get_go_package(source_file);
            let tgt_pkg = get_go_package(target_file);
            if source_file.parent() == target_file.parent() {
                Ok((
                    source_text.to_string(),
                    Some(format!("same package `{src_pkg}`: direct access")),
                ))
            } else {
                let (new_text, note) = rewrite_go_cross_pkg_in_source(
                    source_text,
                    decl_name,
                    &tgt_pkg,
                    target_file,
                    root,
                )?;
                Ok((new_text, (!note.is_empty()).then_some(note)))
            }
        }
        Language::Cpp | Language::C => {
            let rel = display_relative_or_name(source_file, target_file);
            let include_line = format!("#include \"{rel}\"");
            if source_text.contains(&include_line) {
                Ok((source_text.to_string(), None))
            } else {
                let new_text = format!("{include_line}\n{source_text}");
                Ok((new_text, Some(format!("included \"{rel}\""))))
            }
        }
        Language::Swift => Ok((
            source_text.to_string(),
            Some("same module: direct access".to_string()),
        )),
        Language::Rust => Ok((source_text.to_string(), None)),
        Language::Java => Ok((source_text.to_string(), None)),
    }
}

fn remove_from_braced_ts_import(line: &str, sym: &str) -> (String, bool) {
    let Some(open) = line.find('{') else {
        return (line.to_string(), false);
    };
    let Some(close) = line.find('}') else {
        return (line.to_string(), false);
    };
    if open >= close {
        return (line.to_string(), false);
    }
    let inside = &line[open + 1..close];
    let items: Vec<&str> = inside
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    let mut new_items = Vec::new();
    let mut removed = false;
    for item in items {
        let first_word = item.split_whitespace().next().unwrap_or("");
        if first_word == sym {
            removed = true;
        } else {
            new_items.push(item);
        }
    }
    if !removed {
        return (line.to_string(), false);
    }
    if new_items.is_empty() {
        return (String::new(), true);
    }
    let indent = &line[..line.len() - line.trim_start().len()];
    let after_close = &line[close + 1..];
    let rejoined = format!("{indent}import {{ {} }}{after_close}", new_items.join(", "));
    (rejoined, true)
}

fn remove_from_py_from_import(line: &str, sym: &str) -> (String, bool) {
    let Some(pos) = line.find(" import ") else {
        return (line.to_string(), false);
    };
    let before = &line[..pos + 8];
    let after = &line[pos + 8..];
    let items: Vec<&str> = after
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    let mut new_items = Vec::new();
    let mut removed = false;
    for item in items {
        let first_word = item.split_whitespace().next().unwrap_or("");
        if first_word == sym {
            removed = true;
        } else {
            new_items.push(item);
        }
    }
    if !removed {
        return (line.to_string(), false);
    }
    if new_items.is_empty() {
        return (String::new(), true);
    }
    let rejoined = format!("{before}{}", new_items.join(", "));
    (rejoined, true)
}

pub fn rewrite_caller_imports(
    caller_text: &str,
    decl_name: &str,
    caller_file: &Path,
    source_file: &Path,
    target_file: &Path,
    root: &Path,
    lang: Language,
) -> Option<(String, String)> {
    match lang {
        Language::TypeScript | Language::JavaScript => {
            let rel_to_source = relative_import_specifier(caller_file, source_file);
            let rel_to_target = relative_import_specifier(caller_file, target_file);

            let mut found = false;
            let mut lines = Vec::new();
            let mut notes = Vec::new();

            for line in caller_text.lines() {
                let trimmed = line.trim();
                let matches_source = trimmed.starts_with("import ")
                    && (trimmed.contains(&rel_to_source)
                        || trimmed.contains(&rel_to_source.replace("./", "")));
                if matches_source && is_symbol_used(line, decl_name) {
                    found = true;
                    let (shrunk, _) = remove_from_braced_ts_import(line, decl_name);
                    if shrunk.is_empty() {
                        let rewritten_import = line
                            .replace(&rel_to_source, &rel_to_target)
                            .replace(&rel_to_source.replace("./", ""), &rel_to_target);
                        lines.push(rewritten_import);
                        notes.push(format!("rewrote import to `{rel_to_target}`"));
                    } else {
                        lines.push(shrunk);
                        lines.push(format!("import {{ {decl_name} }} from \"{rel_to_target}\";"));
                        notes.push(format!("imported `{decl_name}` from `{rel_to_target}`"));
                    }
                    continue;
                }
                lines.push(line.to_string());
            }

            if found {
                let mut out = lines.join("\n");
                if caller_text.ends_with('\n') {
                    out.push('\n');
                }
                return Some((out, notes.join("; ")));
            }

            if is_symbol_used(caller_text, decl_name) && !caller_text.contains(&rel_to_target) {
                let import_line = format!("import {{ {decl_name} }} from \"{rel_to_target}\";\n");
                let mut out = import_line;
                out.push_str(caller_text);
                return Some((
                    out,
                    format!("added import `{decl_name}` from `{rel_to_target}`"),
                ));
            }

            None
        }
        Language::Python => {
            let src_mod = python_module_specifier(caller_file, source_file, root);
            let tgt_mod = python_module_specifier(caller_file, target_file, root);

            let mut found = false;
            let mut lines = Vec::new();
            let mut notes = Vec::new();

            for line in caller_text.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("from ")
                    && trimmed.contains(&src_mod)
                    && is_symbol_used(line, decl_name)
                {
                    found = true;
                    let (shrunk, _) = remove_from_py_from_import(line, decl_name);
                    if shrunk.is_empty() {
                        let replaced = line.replace(&src_mod, &tgt_mod);
                        lines.push(replaced);
                        notes.push(format!("from {tgt_mod} import {decl_name}"));
                    } else {
                        lines.push(shrunk);
                        lines.push(format!("from {tgt_mod} import {decl_name}"));
                        notes.push(format!("from {tgt_mod} import {decl_name}"));
                    }
                    continue;
                }
                lines.push(line.to_string());
            }

            if found {
                let mut out = lines.join("\n");
                if caller_text.ends_with('\n') {
                    out.push('\n');
                }
                return Some((out, notes.join("; ")));
            }

            None
        }
        Language::Go => {
            let src_pkg = get_go_package(source_file);
            let tgt_pkg = get_go_package(target_file);
            if src_pkg == tgt_pkg {
                return None;
            }
            let old_call = format!("{src_pkg}.{decl_name}");
            let new_call = format!("{tgt_pkg}.{decl_name}");
            if caller_text.contains(&old_call) {
                let mut replaced = caller_text.replace(&old_call, &new_call);
                let src_pkg_path = display(root, source_file.parent().unwrap_or(root));
                let tgt_pkg_path = display(root, target_file.parent().unwrap_or(root));
                if replaced.contains(&src_pkg_path) {
                    replaced = replaced.replace(&src_pkg_path, &tgt_pkg_path);
                }
                return Some((replaced, format!("rewrote `{old_call}` to `{new_call}`")));
            }
            None
        }
        Language::Cpp | Language::C => {
            let src_hdr = display_relative_or_name(caller_file, source_file);
            let tgt_hdr = display_relative_or_name(caller_file, target_file);
            let old_inc = format!("#include \"{src_hdr}\"");
            let new_inc = format!("#include \"{tgt_hdr}\"");
            if caller_text.contains(&old_inc) && !caller_text.contains(&new_inc) {
                let replaced = caller_text.replace(&old_inc, &format!("{old_inc}\n{new_inc}"));
                return Some((replaced, format!("included \"{tgt_hdr}\"")));
            }
            None
        }
        _ => None,
    }
}

fn initial_file_header(target: &Path, lang: Language) -> String {
    match lang {
        Language::Go => {
            let pkg = target
                .parent()
                .and_then(|dir| {
                    if let Ok(entries) = std::fs::read_dir(dir) {
                        for e in entries.flatten() {
                            let p = e.path();
                            if p.extension().is_some_and(|ext| ext == "go")
                                && p != target
                                && let Ok(content) = std::fs::read_to_string(&p)
                            {
                                for line in content.lines() {
                                    let t = line.trim();
                                    if let Some(rest) = t.strip_prefix("package ") {
                                        let name = rest.trim();
                                        if !name.is_empty() {
                                            return Some(name.to_string());
                                        }
                                    }
                                }
                            }
                        }
                    }
                    dir.file_name().map(|n| n.to_string_lossy().into_owned())
                })
                .unwrap_or_else(|| "main".to_string());
            format!("package {pkg}\n\n")
        }
        Language::Cpp | Language::C => {
            if let Some(ext) = target.extension().and_then(|e| e.to_str())
                && matches!(ext, "h" | "hpp" | "hxx")
            {
                "#pragma once\n\n".to_string()
            } else {
                String::new()
            }
        }
        _ => String::new(),
    }
}

fn cpp_move_target_is_implementation(lang: Language, target: &Path) -> bool {
    matches!(lang, Language::Cpp | Language::C)
        && matches!(
            target.extension().and_then(|ext| ext.to_str()),
            Some("cpp" | "cc" | "cxx" | "c")
        )
}

#[allow(clippy::too_many_arguments)]
pub async fn move_item(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    _col: u32,
    target: &Path,
    apply: bool,
    force: bool,
) -> Result<Move> {
    anyhow::ensure!(
        file != target,
        "the item is already in {}",
        display(root, target)
    );

    let lang = Language::of(file)
        .with_context(|| format!("unsupported source language for {}", file.display()))?;
    let target_lang = Language::of(target)
        .with_context(|| format!("unsupported target language for {}", target.display()))?;

    anyhow::ensure!(
        is_compatible_language_family(lang, target_lang),
        "cannot move between incompatible languages ({} and {})",
        lang.fence(),
        target_lang.fence()
    );
    if cpp_move_target_is_implementation(lang, target) {
        anyhow::bail!(
            "cannot move a C/C++ declaration into an implementation file: callers cannot safely include a `.cpp`/`.c` file; move it to a header instead"
        );
    }

    let source_text = std::fs::read_to_string(file)
        .with_context(|| format!("cannot read {}", file.display()))?;

    let target_existed = target.exists();
    let target_text = if target_existed {
        std::fs::read_to_string(target)
            .with_context(|| format!("cannot read {}", target.display()))?
    } else {
        String::new()
    };

    let (name, decl_start, decl_end) = if let Ok(symbols) =
        crate::move_item::document_symbols(remote, root, file).await
        && let Some((s_name, s, e)) = crate::move_item::span_at(&symbols, line)
        && !s_name.is_empty()
    {
        (s_name, s, e)
    } else {
        find_polyglot_decl(&source_text, line, lang)?
    };

    if target_existed {
        check_target_collision(&target_text, &name, target_lang)?;
    }

    let start = with_doc_comment_polyglot(&source_text, decl_start, lang);
    let (source_new_cut, item_raw) = crate::move_item::cut(&source_text, start, decl_end);
    let item = format_item_for_target(&item_raw, lang);

    let (target_with_carried, carried_notes) =
        carry_imports_polyglot(&source_text, &item, &target_text, file, target, lang, root);

    let target_new = if target_with_carried.trim().is_empty() {
        let header = initial_file_header(target, lang);
        if header.is_empty() {
            format!("{}\n", item.trim())
        } else {
            format!("{header}{}\n", item.trim())
        }
    } else {
        crate::move_item::append_item(&target_with_carried, &item)
    };

    let (source_new, source_import_note) =
        update_source_imports(&source_new_cut, &name, file, target, root, lang)?;

    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    rewritten.insert(file.to_path_buf(), source_new);
    rewritten.insert(target.to_path_buf(), target_new);

    let mut import_notes = Vec::new();
    for note in carried_notes {
        import_notes.push(format!("{}: {note}", display(root, target)));
    }
    if let Some(note) = source_import_note {
        import_notes.push(format!("{}: {note}", display(root, file)));
    }

    let workspace_sources = crate::signature_polyglot::collect_workspace_sources(root, lang);
    for src in workspace_sources {
        if src == file || src == target {
            continue;
        }
        let Ok(caller_text) = std::fs::read_to_string(&src) else {
            continue;
        };
        if let Some((rewritten_caller, note)) =
            rewrite_caller_imports(&caller_text, &name, &src, file, target, root, lang)
        {
            import_notes.push(format!("{}: {note}", display(root, &src)));
            rewritten.insert(src, rewritten_caller);
        }
    }

    let edits: Vec<(PathBuf, String)> = rewritten
        .iter()
        .map(|(p, t)| (p.clone(), t.clone()))
        .collect();
    let reports = crate::diagnostics::validate_texts(remote, root, &edits, &[]).await?;
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
            diagnostics.is_empty() || force,
            "the move does not compile ({} error(s)); nothing was written. Fix the request, or              pass `force: true` to write it anyway:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        let edit = crate::signature::whole_file_edit(&rewritten);
        crate::refactor::apply_workspace_edit(root, &edit)?;
        applied = true;
    }

    let from_mod = module_display_name(file, root, lang);
    let to_mod = module_display_name(target, root, lang);
    let new_path = match lang {
        Language::Python => format!("{to_mod}.{name}"),
        Language::Go => format!("{to_mod}.{name}"),
        Language::Swift => name.clone(),
        _ => format!("{to_mod}::{name}"),
    };

    let created = if target_existed {
        None
    } else {
        Some((
            display(root, target),
            display(root, target.parent().unwrap_or(root)),
        ))
    };

    Ok(Move {
        symbol: name,
        root: root.to_path_buf(),
        from: display(root, file),
        from_module: from_mod,
        to: display(root, target),
        to_module: to_mod,
        new_path,
        moved_lines: (decl_end - start + 1) as usize,
        rewritten: rewritten
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        imports: import_notes,
        left_alone: Vec::new(),
        unmatched: Vec::new(),
        diagnostics,
        applied,
        created,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_import_specifiers_compute_accurately() {
        assert_eq!(
            relative_import_specifier(Path::new("src/features/foo.ts"), Path::new("src/common/utils.ts")),
            "../common/utils"
        );
        assert_eq!(
            relative_import_specifier(Path::new("src/index.ts"), Path::new("src/utils.ts")),
            "./utils"
        );
        assert_eq!(
            relative_import_specifier(Path::new("index.ts"), Path::new("utils.ts")),
            "./utils"
        );
    }

    #[test]
    fn python_module_specifiers_compute_accurately() {
        let root = Path::new("/workspace");
        assert_eq!(
            python_module_specifier(
                Path::new("/workspace/pkg/utils.py"),
                Path::new("/workspace/pkg/helpers.py"),
                root
            ),
            "pkg.helpers"
        );
    }

    #[test]
    fn find_polyglot_decl_typescript_function() {
        let ts = "/**\n * Helper\n */\nexport function add(a: number, b: number): number {\n    return a + b;\n}\n";
        let (name, start, end) = find_polyglot_decl(ts, 4, Language::TypeScript).unwrap();
        assert_eq!(name, "add");
        assert_eq!(start, 1);
        assert_eq!(end, 6);
    }

    #[test]
    fn find_polyglot_decl_python_function() {
        let py = "@deco\ndef helper(x):\n    return x * 2\n";
        let (name, start, end) = find_polyglot_decl(py, 2, Language::Python).unwrap();
        assert_eq!(name, "helper");
        assert_eq!(start, 1);
        assert_eq!(end, 3);
    }

    #[test]
    fn find_polyglot_decl_go_function() {
        let go = "// Add doc\nfunc Add(a, b int) int {\n\treturn a + b\n}\n";
        let (name, start, end) = find_polyglot_decl(go, 2, Language::Go).unwrap();
        assert_eq!(name, "Add");
        assert_eq!(start, 1);
        assert_eq!(end, 4);
    }

    #[test]
    fn format_item_adds_export_in_typescript() {
        let item = "function calculate(x: number) {\n    return x;\n}";
        let formatted = format_item_for_target(item, Language::TypeScript);
        assert!(formatted.starts_with("export function calculate"));
    }

    #[test]
    fn remove_from_braced_ts_import_works() {
        let line = "import { A, B } from \"./foo\";";
        let (shrunk, removed) = remove_from_braced_ts_import(line, "A");
        assert!(removed);
        assert_eq!(shrunk, "import { B } from \"./foo\";");

        let line_single = "import { A } from \"./foo\";";
        let (shrunk2, removed2) = remove_from_braced_ts_import(line_single, "A");
        assert!(removed2);
        assert_eq!(shrunk2, "");
    }

    #[test]
    fn go_call_rewriting_skips_comments_strings_and_identifier_substrings() {
        let source = "package p\nfunc use() {\n Add(1)\n _ = \"Add failed\"\n // Add remains a comment\n AddSuffix()\n}\n";
        let (rewritten, count) = rewrite_go_call_sites(source, "Add", "helpers").unwrap();

        assert_eq!(count, 1);
        assert!(rewritten.contains("helpers.Add(1)"));
        assert!(rewritten.contains("\"Add failed\""));
        assert!(rewritten.contains("// Add remains a comment"));
        assert!(rewritten.contains("AddSuffix()"));
    }

    #[test]
    fn go_import_path_keeps_the_module_prefix() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("go.mod"), "module example.com/app\n").unwrap();
        let target = temp.path().join("internal/helpers");
        std::fs::create_dir_all(&target).unwrap();

        assert_eq!(
            go_module_import_path(&target).unwrap(),
            "example.com/app/internal/helpers"
        );
    }

    #[test]
    fn cpp_moves_reject_implementation_file_targets() {
        assert!(cpp_move_target_is_implementation(
            Language::Cpp,
            Path::new("helpers.cpp")
        ));
        assert!(!cpp_move_target_is_implementation(
            Language::Cpp,
            Path::new("helpers.hpp")
        ));
        assert!(!cpp_move_target_is_implementation(
            Language::TypeScript,
            Path::new("helpers.cpp")
        ));
    }
}
