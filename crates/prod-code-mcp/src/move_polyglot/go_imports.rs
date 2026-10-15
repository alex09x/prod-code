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

use anyhow::Result;

use crate::parameter_object::Language;

use super::specifiers::is_ident;

pub(crate) fn go_import_package(spec: &str) -> Option<String> {
    let spec = spec
        .split_once("//")
        .map_or(spec, |(before, _)| before)
        .trim();
    let words: Vec<&str> = spec.split_whitespace().collect();
    let (alias, path) = match words.as_slice() {
        [path] => (None, *path),
        [alias, path] if is_ident(alias.chars().next()?) && alias.chars().all(is_ident) => {
            (Some(*alias), *path)
        }
        _ => return None,
    };
    if matches!(alias, Some("." | "_")) || !path.starts_with('"') || !path.ends_with('"') {
        return None;
    }
    let import_path = &path[1..path.len() - 1];
    if import_path == "C" {
        return None;
    }
    let package = match alias {
        Some(alias) => alias.to_string(),
        None if !import_path.contains('/') => import_path.to_string(),
        // Without an explicit alias, a path's last component need not be its package name.
        None => return None,
    };
    package.chars().all(is_ident).then_some(package)
}

fn scan_go_prefix(content: &str, at: usize) -> (bool, Option<(char, usize)>) {
    let mut stack: Vec<(char, usize)> = Vec::new();
    let mut chars = content[..at].char_indices().peekable();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut line_comment = false;
    let mut block_comment = false;
    while let Some((idx, ch)) = chars.next() {
        if line_comment {
            if ch == '\n' {
                line_comment = false;
            }
            continue;
        }
        if block_comment {
            if ch == '*' && chars.peek().map(|(_, c)| *c) == Some('/') {
                chars.next();
                block_comment = false;
            }
            continue;
        }
        if let Some(delim) = quote {
            if delim == '`' {
                if ch == '`' {
                    quote = None;
                }
            } else if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == delim {
                quote = None;
            }
            continue;
        }
        if ch == '/' && chars.peek().map(|(_, c)| *c) == Some('/') {
            chars.next();
            line_comment = true;
        } else if ch == '/' && chars.peek().map(|(_, c)| *c) == Some('*') {
            chars.next();
            block_comment = true;
        } else if matches!(ch, '\'' | '"' | '`') {
            quote = Some(ch);
        } else if matches!(ch, '(' | '[' | '{') {
            stack.push((ch, idx));
        } else if matches!(ch, ')' | ']' | '}') {
            stack.pop();
        }
    }
    (
        quote.is_some() || line_comment || block_comment,
        stack.pop(),
    )
}

pub(crate) fn is_in_go_string_or_comment(content: &str, at: usize) -> bool {
    scan_go_prefix(content, at).0
}

pub(crate) fn go_package_is_used(content: &str, package: &str) -> bool {
    content.match_indices(package).any(|(at, _)| {
        let end = at + package.len();
        !(at > 0 && content[..at].chars().next_back().is_some_and(is_ident))
            && !content[end..].chars().next().is_some_and(is_ident)
            && !is_in_go_string_or_comment(content, at)
            && content[end..].trim_start().starts_with('.')
    })
}

pub(crate) fn remove_unused_go_imports(source: &str) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let mut output = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim() == "import (" {
            let Some(close) = (i + 1..lines.len()).find(|&at| lines[at].trim() == ")") else {
                output.push(lines[i].to_string());
                i += 1;
                continue;
            };
            let specs: Option<Vec<(&str, String)>> = lines[i + 1..close]
                .iter()
                .map(|line| go_import_package(line).map(|package| (*line, package)))
                .collect();
            if let Some(specs) = specs {
                let retained: Vec<_> = specs
                    .into_iter()
                    .filter(|(_, package)| go_package_is_used(source, package))
                    .collect();
                if !retained.is_empty() {
                    output.push(lines[i].to_string());
                    output.extend(retained.into_iter().map(|(line, _)| line.to_string()));
                    output.push(lines[close].to_string());
                }
                i = close + 1;
                continue;
            }
        }
        if let Some(spec) = lines[i].trim().strip_prefix("import ")
            && let Some(package) = go_import_package(spec)
            && !go_package_is_used(source, &package)
        {
            i += 1;
            continue;
        }
        output.push(lines[i].to_string());
        i += 1;
    }
    let mut result = output.join("\n");
    if source.ends_with('\n') {
        result.push('\n');
    }
    result
}

pub(crate) fn get_go_package(file: &Path) -> String {
    if let Ok(content) = std::fs::read_to_string(file) {
        for line in content.lines() {
            let t = line.trim();
            if let Some(rest) = t.strip_prefix("package ") {
                let name = rest
                    .split("//")
                    .next()
                    .unwrap_or(rest)
                    .trim()
                    .trim_end_matches(';');
                if !name.is_empty() {
                    return name.to_string();
                }
            }
        }
    }
    let header = super::target::initial_file_header(file, Language::Go);
    for line in header.lines() {
        if let Some(rest) = line.trim().strip_prefix("package ") {
            let name = rest.trim();
            if !name.is_empty() {
                return name.to_string();
            }
        }
    }
    file.parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "main".to_string())
}

pub(crate) fn go_module_import_path(target_dir: &Path) -> Result<String> {
    let target = std::fs::canonicalize(target_dir).unwrap_or_else(|_| target_dir.to_path_buf());
    let mut directory = Some(target.as_path());
    while let Some(dir) = directory {
        if let Ok(manifest) = std::fs::read_to_string(dir.join("go.mod"))
            && let Some(module) = manifest.lines().find_map(|line| {
                line.trim()
                    .strip_prefix("module ")
                    .map(|value| {
                        value
                            .split("//")
                            .next()
                            .unwrap_or(value)
                            .trim()
                            .trim_matches('"')
                            .to_string()
                    })
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

fn is_short_var_decl_lhs(after_on_line: &str) -> bool {
    let mut rest = after_on_line.trim_start();
    loop {
        if rest.starts_with(":=") {
            return true;
        }
        if let Some(after_comma) = rest.strip_prefix(',') {
            let after_ws = after_comma.trim_start();
            let ident_len = after_ws.chars().take_while(|&c| is_ident(c)).count();
            if ident_len == 0 {
                return false;
            }
            rest = after_ws[ident_len..].trim_start();
        } else {
            return false;
        }
    }
}

fn is_go_value_before_colon(source_text: &str, at: usize, before_on_line: &str) -> bool {
    if before_on_line.starts_with("case ") || before_on_line.contains("{ case ") {
        return true;
    }
    match scan_go_prefix(source_text, at).1 {
        Some(('[', _)) => true,
        Some(('{', brace_idx)) => {
            let before_brace = source_text[..brace_idx].trim_end();
            before_brace.contains("map[")
                && before_brace
                    .rfind("map[")
                    .is_some_and(|m| !before_brace[m..].contains('{'))
        }
        _ => false,
    }
}

pub(crate) fn rewrite_go_call_sites(
    source_text: &str,
    decl_name: &str,
    target_pkg: &str,
) -> Result<(String, usize)> {
    if decl_name.is_empty() {
        return Ok((source_text.to_string(), 0));
    }
    let mut edits = Vec::new();
    for (at, _) in source_text.match_indices(decl_name) {
        if (at > 0 && source_text[..at].chars().next_back().is_some_and(is_ident))
            || source_text[at + decl_name.len()..]
                .chars()
                .next()
                .is_some_and(is_ident)
            || is_in_go_string_or_comment(source_text, at)
        {
            continue;
        }
        let line_start = source_text[..at].rfind('\n').map_or(0, |i| i + 1);
        let line_end = source_text[at..]
            .find('\n')
            .map_or(source_text.len(), |i| at + i);
        let before_on_line = source_text[line_start..at].trim_start();
        let before = source_text[..at].trim_end();
        if before_on_line.starts_with("package ") || before.ends_with('.') {
            continue;
        }
        let after = source_text[at + decl_name.len()..].trim_start();
        let after_on_line = source_text[at + decl_name.len()..line_end].trim_start();

        if before_on_line.trim_end() == "func"
            || (before_on_line.starts_with("func ")
                && before_on_line
                    .trim_start_matches("func")
                    .trim_start()
                    .starts_with('(')
                && before_on_line.trim_end().ends_with(')')
                && after.starts_with('('))
        {
            continue; // Separate function or receiver-method declaration.
        }

        let prev_word = before.rsplit(|c: char| !is_ident(c)).next().unwrap_or("");
        if matches!(prev_word, "var" | "const" | "type") {
            anyhow::bail!(
                "Go reference `{decl_name}` at byte {at} is shadowed by a local `{prev_word}` declaration; cannot safely qualify it"
            );
        }

        if is_short_var_decl_lhs(after_on_line) {
            anyhow::bail!(
                "Go reference `{decl_name}` at byte {at} is shadowed by a local binding; cannot safely qualify it"
            );
        }

        if after_on_line.starts_with("...")
            || after_on_line.starts_with("[]")
            || after_on_line
                .chars()
                .next()
                .is_some_and(|c| c.is_alphabetic() || c == '_')
        {
            if before_on_line.starts_with("func ") || before_on_line.contains("func(") {
                anyhow::bail!(
                    "Go reference `{decl_name}` at byte {at} is shadowed by a parameter declaration; cannot safely qualify it"
                );
            }
            continue; // Struct field declaration e.g. `Field Type`.
        }

        if after.starts_with(':')
            && !after.starts_with(":=")
            && !is_go_value_before_colon(source_text, at, before_on_line)
        {
            continue; // Composite-literal field key or statement label.
        }

        if !decl_name.chars().next().is_some_and(char::is_uppercase) {
            anyhow::bail!(
                "Go symbol `{decl_name}` at byte {at} is unexported and cannot be qualified across packages"
            );
        }

        edits.push((at, decl_name.len()));
    }
    let count = edits.len();
    let mut rewritten = source_text.to_string();
    for (at, len) in edits.into_iter().rev() {
        rewritten.replace_range(at..at + len, &format!("{target_pkg}.{decl_name}"));
    }
    Ok((rewritten, count))
}

pub(crate) fn add_go_import(content: &str, pkg: &str) -> String {
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

pub(crate) fn rewrite_go_cross_pkg_in_source(
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
