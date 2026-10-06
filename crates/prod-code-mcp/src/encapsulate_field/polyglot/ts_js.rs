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

use super::super::case::to_pascal_case;
use super::replace::{replace_line_this, replace_line_this_private};

pub fn rewrite_external_ts(code: &str, field: &str) -> (String, usize, usize) {
    let pascal = to_pascal_case(field);
    let mut reads = 0;
    let mut writes = 0;
    let mut out = String::new();
    for line in code.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with('*') {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let needle = format!(".{field}");
        if !line.contains(&needle) {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let mut new_line = String::new();
        let mut rest = line;
        while let Some(pos) = rest.find(&needle) {
            let before = &rest[..pos];
            let after = &rest[pos + needle.len()..];
            let before_trimmed = before.trim_end();
            if before_trimmed.ends_with("this") || before_trimmed.ends_with("this._") {
                new_line.push_str(&rest[..pos + needle.len()]);
                rest = after;
                continue;
            }
            let trimmed_after = after.trim_start();
            if trimmed_after.starts_with('(') {
                new_line.push_str(&rest[..pos + needle.len()]);
                rest = after;
                continue;
            }
            if let Some(c) = after.chars().next()
                && (c.is_alphanumeric() || c == '_')
            {
                new_line.push_str(&rest[..pos + needle.len()]);
                rest = after;
                continue;
            }
            if trimmed_after.starts_with('=')
                && !trimmed_after.starts_with("==")
                && !trimmed_after.starts_with("=>")
            {
                writes += 1;
                let rhs_with_sep = trimmed_after[1..].trim_start();
                let (rhs, sep) = if let Some(semi_pos) = rhs_with_sep.find(';') {
                    (&rhs_with_sep[..semi_pos], &rhs_with_sep[semi_pos..])
                } else {
                    (rhs_with_sep, "")
                };
                new_line.push_str(before);
                new_line.push_str(&format!(".set{pascal}({rhs}){sep}"));
                rest = "";
                break;
            } else {
                reads += 1;
                new_line.push_str(before);
                new_line.push_str(&format!(".get{pascal}()"));
                rest = after;
            }
        }
        new_line.push_str(rest);
        out.push_str(&new_line);
        out.push('\n');
    }
    if !code.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }
    (out, reads, writes)
}

pub fn encapsulate_field_ts(
    text: &str,
    target_class: Option<&str>,
    target_field: &str,
) -> Result<(String, String, String, usize, usize, usize)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut class_start = None;
    let mut class_end = None;
    let mut class_name = String::new();
    let mut brace_depth = 0i32;

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("class ")
            || trimmed.starts_with("export class ")
            || trimmed.starts_with("export default class ")
        {
            let words: Vec<&str> = trimmed.split_whitespace().collect();
            let mut name = "";
            for (w_idx, w) in words.iter().enumerate() {
                if *w == "class" && w_idx + 1 < words.len() {
                    name = words[w_idx + 1].trim_matches('{').trim();
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
            brace_depth += line.chars().filter(|&c| c == '{').count() as i32;
            brace_depth -= line.chars().filter(|&c| c == '}').count() as i32;
            if brace_depth == 0 && line.contains('}') {
                class_end = Some(idx);
                break;
            }
        }
    }

    let c_start = class_start.context("Could not find class in file")?;
    let c_end = class_end.context("Could not find closing brace of class")?;

    let mut field_line_idx = None;
    let mut field_type = String::new();
    let mut field_default = None;
    let mut field_indent = "    ".to_string();

    for (idx, line) in lines.iter().enumerate().take(c_end).skip(c_start + 1) {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with('*') {
            continue;
        }
        let words: Vec<&str> = trimmed.split_whitespace().collect();
        let is_match = words.iter().any(|w| {
            let clean = w.trim_matches(|c| c == ':' || c == ';' || c == '=');
            clean == target_field
        });
        if is_match && !trimmed.contains('(') {
            field_line_idx = Some(idx);
            let indent_len = line.len() - trimmed.len();
            field_indent = line[..indent_len].to_string();
            if let Some(colon_pos) = trimmed.find(':') {
                let after_colon = &trimmed[colon_pos + 1..];
                let ty_end = after_colon
                    .find('=')
                    .or_else(|| after_colon.find(';'))
                    .unwrap_or(after_colon.len());
                field_type = after_colon[..ty_end].trim().to_string();
            }
            if let Some(eq_pos) = trimmed.find('=') {
                let after_eq = &trimmed[eq_pos + 1..];
                let def_end = after_eq.find(';').unwrap_or(after_eq.len());
                field_default = Some(after_eq[..def_end].trim().to_string());
            }
            break;
        }
    }

    let f_idx = field_line_idx
        .with_context(|| format!("Field `{target_field}` not found in class `{class_name}`"))?;
    let type_colon = if field_type.is_empty() {
        String::new()
    } else {
        format!(": {field_type}")
    };
    let default_eq = field_default.map(|d| format!(" = {d}")).unwrap_or_default();
    let new_field_line = format!("{field_indent}private _{target_field}{type_colon}{default_eq};");

    let pascal = to_pascal_case(target_field);
    let accessors = format!(
        "\n{field_indent}public get{pascal}(){type_colon} {{\n{field_indent}    return this._{target_field};\n{field_indent}}}\n\n{field_indent}public set{pascal}({target_field}{type_colon}): void {{\n{field_indent}    this._{target_field} = {target_field};\n{field_indent}}}\n"
    );

    let mut out_lines = Vec::new();
    let mut left_direct = 0;
    let mut reads = 0;
    let mut writes = 0;
    for (idx, line) in lines.iter().enumerate() {
        if idx == f_idx {
            out_lines.push(new_field_line.clone());
        } else if idx > c_start && idx < c_end {
            let (replaced_line, count) = replace_line_this(line, target_field);
            left_direct += count;
            let (replaced_line, r, w) = rewrite_external_ts(&replaced_line, target_field);
            reads += r;
            writes += w;
            out_lines.push(replaced_line);
        } else if idx == c_end {
            out_lines.push(accessors.clone());
            out_lines.push(line.to_string());
        } else {
            let (replaced, r, w) = rewrite_external_ts(line, target_field);
            reads += r;
            writes += w;
            out_lines.push(replaced);
        }
    }

    let final_code = out_lines.join("\n");
    Ok((
        class_name,
        field_type,
        final_code,
        reads,
        writes,
        left_direct,
    ))
}

/// JavaScript uses private class fields and ordinary methods rather than TypeScript modifiers
/// and annotations. External accesses keep the same method-call form used by the other
/// generators.
pub fn encapsulate_field_js(
    text: &str,
    target_class: Option<&str>,
    target_field: &str,
) -> Result<(String, String, String, usize, usize, usize)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut class_start = None;
    let mut class_end = None;
    let mut class_name = String::new();
    let mut brace_depth = 0i32;

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("class ")
            || trimmed.starts_with("export class ")
            || trimmed.starts_with("export default class ")
        {
            let words: Vec<&str> = trimmed.split_whitespace().collect();
            let name = words
                .iter()
                .enumerate()
                .find_map(|(i, word)| {
                    (*word == "class")
                        .then(|| words.get(i + 1).copied())
                        .flatten()
                })
                .unwrap_or("")
                .trim_matches('{')
                .trim();
            if target_class.is_none() || target_class == Some(name) {
                class_start = Some(idx);
                class_name = name.to_string();
                brace_depth = 0;
            }
        }
        if class_start.is_some() && class_end.is_none() {
            brace_depth += line.chars().filter(|&c| c == '{').count() as i32;
            brace_depth -= line.chars().filter(|&c| c == '}').count() as i32;
            if brace_depth == 0 && line.contains('}') {
                class_end = Some(idx);
                break;
            }
        }
    }

    let c_start = class_start.context("Could not find class in JavaScript file")?;
    let c_end = class_end.context("Could not find closing brace of JavaScript class")?;
    let mut field_line_idx = None;
    let mut field_default = String::new();
    let mut field_indent = "    ".to_string();
    for (idx, line) in lines.iter().enumerate().take(c_end).skip(c_start + 1) {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with('*') {
            continue;
        }
        let words: Vec<&str> = trimmed.split_whitespace().collect();
        let is_match = words
            .iter()
            .any(|w| w.trim_matches(|c| c == ';' || c == '=' || c == ',') == target_field);
        if is_match && !trimmed.contains('(') {
            field_line_idx = Some(idx);
            field_indent = line[..line.len() - trimmed.len()].to_string();
            if let Some(eq) = trimmed.find('=') {
                let value = trimmed[eq + 1..].trim().trim_end_matches(';').trim();
                field_default = format!(" = {value}");
            }
            break;
        }
    }
    let f_idx = field_line_idx.with_context(|| {
        format!("Field `{target_field}` not found in JavaScript class `{class_name}`")
    })?;
    let pascal = to_pascal_case(target_field);
    let new_field = format!("{field_indent}#{target_field}{field_default};");
    let accessors = format!(
        "\n{field_indent}get{pascal}() {{\n{field_indent}    return this.#{target_field};\n{field_indent}}}\n\n{field_indent}set{pascal}({target_field}) {{\n{field_indent}    this.#{target_field} = {target_field};\n{field_indent}}}\n"
    );
    let mut out_lines = Vec::new();
    let mut reads = 0;
    let mut writes = 0;
    let mut internal = 0;
    for (idx, line) in lines.iter().enumerate() {
        if idx == f_idx {
            out_lines.push(new_field.clone());
        } else if idx > c_start && idx < c_end {
            let (replaced, count) = replace_line_this_private(line, target_field);
            internal += count;
            let (replaced, r, w) = rewrite_external_ts(&replaced, target_field);
            reads += r;
            writes += w;
            out_lines.push(replaced);
        } else if idx == c_end {
            out_lines.push(accessors.clone());
            out_lines.push(line.to_string());
        } else {
            let (replaced, r, w) = rewrite_external_ts(line, target_field);
            reads += r;
            writes += w;
            out_lines.push(replaced);
        }
    }
    let mut final_code = out_lines.join("\n");
    if !text.ends_with('\n') && final_code.ends_with('\n') {
        final_code.pop();
    }
    Ok((
        class_name,
        String::new(),
        final_code,
        reads,
        writes,
        internal,
    ))
}
