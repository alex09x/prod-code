/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result};

use super::rewrite::rewrite_static_calls_in_code;
use crate::make_static::Language;

#[path = "swift_scope.rs"]
mod scope;
#[path = "swift_unit.rs"]
mod unit;

/// A Void method needs no implicit return when its receiver alias is inserted.
/// Inspect the signature suffix, excluding comments and nested type arrows.
/// Resolve unit spellings only when their names are unshadowed at this method.
pub(super) fn returns_void(suffix: &str, source: &str, method: usize, body: &str) -> bool {
    let syntax = super::binding::mask(suffix, Language::Swift);
    let mut depth = 0;
    for (i, byte) in suffix.bytes().enumerate() {
        if !syntax[i] {
            continue;
        }
        match byte {
            b'(' | b'[' => depth += 1,
            b')' | b']' => depth -= 1,
            b'-' if depth == 0 && suffix.as_bytes().get(i + 1) == Some(&b'>') => {
                let result: String = suffix[i + 2..]
                    .char_indices()
                    .filter(|(at, ch)| syntax[i + 2 + at] && !ch.is_whitespace())
                    .map(|(_, ch)| ch)
                    .collect();
                return match ungroup(&result) {
                    "()" => true,
                    "Void" | "Swift.Void" => {
                        let name = if ungroup(&result) == "Void" {
                            "Void"
                        } else {
                            "Swift"
                        };
                        match shadows(source, method, name) {
                            Some(shadowed) => !shadowed,
                            None => unit::separate_calls(body),
                        }
                    }
                    _ => false,
                };
            }
            _ => {}
        }
    }
    true
}

/// Remove only parentheses enclosing the entire type. Optional, tuple and
/// function types retain their distinguishing syntax after this normalization.
fn ungroup(mut result: &str) -> &str {
    while result != "()" && result.starts_with('(') && result.ends_with(')') {
        let mut depth = 0;
        let whole = result.bytes().enumerate().all(|(i, byte)| {
            match byte {
                b'(' => depth += 1,
                b')' => depth -= 1,
                _ => {}
            }
            depth > 0 || i == result.len() - 1
        });
        if !whole || depth != 0 {
            break;
        }
        result = &result[1..result.len() - 1];
    }
    result
}

// None means an external scope is unknown, not that it binds the name.
fn shadows(source: &str, method: usize, name: &str) -> Option<bool> {
    let syntax = super::binding::mask(source, Language::Swift);
    let tokens = super::binding::words(source, Language::Swift);
    // Bindings may be lexical, nominal/extension members, or inherited members.
    // An unrelated nominal type cannot bind this name.
    let mut scopes: Vec<_> = std::iter::once(0..source.len() + 1).collect();
    let mut stack = vec![0];
    let mut scope_at = vec![0; source.len()];
    for (at, byte) in source.bytes().enumerate() {
        scope_at[at] = *stack.last().unwrap();
        if !syntax[at] {
            continue;
        }
        match byte {
            b'{' => {
                stack.push(scopes.len());
                scopes.push(at + 1..source.len() + 1);
            }
            b'}' if stack.len() > 1 => scopes[stack.pop().unwrap()].end = at + 1,
            _ => {}
        }
    }
    let (extensions, unresolved_base) =
        scope::visible_extensions(source, method, name, &syntax, &tokens, &scopes, &scope_at);
    for pair in tokens.windows(2) {
        let [(_, keyword), (at, word)] = pair else {
            unreachable!()
        };
        let nominal = matches!(*keyword, "class" | "struct" | "enum" | "protocol" | "actor");
        if *word == name
            && (nominal || matches!(*keyword, "typealias" | "associatedtype"))
            && (scopes[scope_at[*at]].contains(&method) || extensions[scope_at[*at]])
        {
            return Some(true);
        }
        if (nominal || *keyword == "func")
            && generic_shadows(source, &syntax, *at + word.len(), method, name, &scopes)
        {
            return Some(true);
        }
    }
    (!unresolved_base).then_some(false)
}

/// Generic names belong to their declaration's body, rather than the scope in
/// which the declaration itself is named. Constraint arguments do not bind names.
fn generic_shadows(
    source: &str,
    syntax: &[bool],
    after_name: usize,
    method: usize,
    name: &str,
    scopes: &[std::ops::Range<usize>],
) -> bool {
    let mut code = source
        .bytes()
        .enumerate()
        .skip(after_name)
        .filter(|(at, byte)| syntax[*at] && !byte.is_ascii_whitespace())
        .peekable();
    if !matches!(code.peek(), Some((_, b'<'))) {
        return false;
    }
    let code: Vec<_> = code.collect();
    let mut depth = 0;
    let mut parameter = false;
    let mut binds = false;
    let mut end = None;
    for (index, &(at, byte)) in code.iter().enumerate() {
        match byte {
            b'<' => {
                depth += 1;
                if depth == 1 {
                    parameter = true;
                }
            }
            b'>' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(index + 1);
                    break;
                }
            }
            b',' if depth == 1 => parameter = true,
            _ if parameter && depth == 1 => {
                if at > 0
                    && (source.as_bytes()[at - 1].is_ascii_alphanumeric()
                        || source.as_bytes()[at - 1] == b'_')
                {
                    continue;
                }
                let word = source[at..]
                    .split(|ch: char| !ch.is_alphanumeric() && ch != '_')
                    .next()
                    .unwrap_or("");
                binds |= word == name;
                // A parameter pack's name follows the `each` keyword.
                parameter = word == "each";
            }
            _ => {}
        }
    }
    if !binds {
        return false;
    }
    let Some(end) = end else { return false };
    let mut delimiters = 0;
    for &(at, byte) in &code[end..] {
        match byte {
            b'(' | b'[' => delimiters += 1,
            b')' | b']' => delimiters -= 1,
            b'{' if delimiters == 0 => {
                return scopes
                    .iter()
                    .any(|scope| scope.start == at + 1 && scope.contains(&method));
            }
            b'}' | b';' if delimiters == 0 => break,
            _ => {}
        }
    }
    false
}

pub fn to_method_swift(
    code: &str,
    target_class: Option<&str>,
    target_method: &str,
) -> Result<(String, String, String, String, String, usize, usize)> {
    let lines: Vec<&str> = code.lines().collect();
    let mut class_start = None;
    let mut class_end = None;
    let mut class_name = String::new();
    let mut brace_depth = 0i32;
    let syntax = super::binding::mask(code, Language::Swift);
    let mut line_start = 0;
    let line_starts: Vec<_> = code
        .split_inclusive('\n')
        .map(|line| {
            let start = line_start;
            line_start += line.len();
            start
        })
        .collect();
    let mut offset = 0;

    for (idx, line) in lines.iter().enumerate() {
        let line_offset = offset;
        offset += line.len()
            + if code.as_bytes().get(line_offset + line.len()) == Some(&b'\r') {
                2
            } else {
                1
            };
        let trimmed = line.trim_start();
        if syntax
            .get(line_offset + line.len() - trimmed.len())
            .copied()
            .unwrap_or(false)
            && (trimmed.starts_with("class ")
                || trimmed.starts_with("struct ")
                || trimmed.starts_with("actor "))
        {
            let words: Vec<&str> = trimmed.split_whitespace().collect();
            let mut name = "";
            for (w_idx, w) in words.iter().enumerate() {
                if (*w == "class" || *w == "struct" || *w == "actor") && w_idx + 1 < words.len() {
                    name = words[w_idx + 1]
                        .trim_matches(|c| c == '{' || c == ':')
                        .trim();
                    break;
                }
            }
            if target_class.is_none() || target_class == Some(name) {
                class_start = Some(idx);
                class_name = name.to_string();
                brace_depth = 0;
            }
        }
        if class_start.is_some() && class_end.is_none() {
            brace_depth += line
                .bytes()
                .enumerate()
                .filter(|(i, c)| *c == b'{' && syntax[line_offset + i])
                .count() as i32;
            brace_depth -= line
                .bytes()
                .enumerate()
                .filter(|(i, c)| *c == b'}' && syntax[line_offset + i])
                .count() as i32;
            if brace_depth == 0 && line.contains('}') {
                class_end = Some(idx);
                break;
            }
        }
    }

    let c_start = class_start.context("Could not find class/struct in Swift file")?;
    let c_end = class_end.context("Could not find closing brace in Swift type")?;

    let mut method_line_idx = None;
    for (idx, line) in lines.iter().enumerate().take(c_end).skip(c_start + 1) {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with('*') {
            continue;
        }
        let needle = format!("func {target_method}(");
        if trimmed
            .find(&needle)
            .is_some_and(|at| syntax[line_starts[idx] + line.len() - trimmed.len() + at])
        {
            method_line_idx = Some(idx);
            break;
        }
    }

    let m_idx = method_line_idx
        .with_context(|| format!("Method `{target_method}` not found in `{class_name}`"))?;

    let m_line = lines[m_idx];
    if !m_line.contains("static func ") && !m_line.contains("class func ") {
        anyhow::bail!("`{target_method}` is already an instance method");
    }

    let trimmed = m_line.trim_start();
    let indent = &m_line[..m_line.len() - trimmed.len()];
    let open_p = trimmed.find('(').context("Missing parameter list")?;
    let close_p = super::binding::parameter_close(trimmed, Language::Swift)?;
    let params_str = &trimmed[open_p + 1..close_p];
    let params = super::swift_regex::arguments(params_str)
        .context("unsupported Swift parameter literals")?;
    let first_param = params
        .first()
        .context("Method takes no parameters; nothing can become `self`")?
        .clone();
    let param_name = first_param
        .split(':')
        .next()
        .unwrap_or(&first_param)
        .split_whitespace()
        .next_back()
        .unwrap_or("")
        .to_string();

    let remaining_params = if params.len() > 1 {
        params[1..].join(", ")
    } else {
        String::new()
    };

    super::binding::parameters(
        &first_param,
        &param_name,
        &remaining_params,
        Language::Swift,
    )?;
    let without_static =
        trimmed
            .replacen("static func ", "func ", 1)
            .replacen("class func ", "func ", 1);
    let new_m_trimmed = if let Some(op) = without_static.find('(') {
        let cp = super::binding::parameter_close(&without_static, Language::Swift)?;
        format!(
            "{}{remaining_params}{}",
            &without_static[..op + 1],
            &without_static[cp..]
        )
    } else {
        without_static
    };

    let (intermediate, renamed_uses) = super::body::braced(
        code,
        m_idx,
        &format!("{indent}{new_m_trimmed}"),
        &param_name,
        &first_param,
        Language::Swift,
    )?;
    let (final_code, rewritten_calls) =
        rewrite_static_calls_in_code(&intermediate, target_method, &class_name, Language::Swift);

    Ok((
        class_name,
        target_method.to_string(),
        first_param,
        "self".to_string(),
        final_code,
        renamed_uses,
        rewritten_calls,
    ))
}
