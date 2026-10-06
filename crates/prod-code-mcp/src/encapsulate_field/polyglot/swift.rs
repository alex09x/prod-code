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
use super::replace::replace_line_self;

pub fn rewrite_external_swift(code: &str, field: &str) -> (String, usize, usize) {
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
            if before_trimmed.ends_with("self") || before_trimmed.ends_with("self._") {
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
            if trimmed_after.starts_with('=') && !trimmed_after.starts_with("==") {
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

pub fn encapsulate_field_swift(
    text: &str,
    target_class: Option<&str>,
    target_field: &str,
) -> Result<(String, String, String, usize, usize, usize)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut class_start = None;
    let mut class_end = None;
    let mut class_name = String::new();
    let mut is_struct = false;
    let mut brace_depth = 0i32;

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("class ")
            || trimmed.starts_with("struct ")
            || trimmed.starts_with("public class ")
            || trimmed.starts_with("public struct ")
        {
            let words: Vec<&str> = trimmed.split_whitespace().collect();
            let mut name = "";
            for (w_idx, w) in words.iter().enumerate() {
                if (*w == "class" || *w == "struct") && w_idx + 1 < words.len() {
                    name = words[w_idx + 1]
                        .trim_matches(|c| c == '{' || c == ':')
                        .trim();
                    is_struct = *w == "struct";
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

    let c_start = class_start.context("Could not find class or struct in Swift file")?;
    let c_end = class_end.context("Could not find closing brace of Swift class")?;

    let mut field_line_idx = None;
    let mut field_type = String::new();
    let mut field_indent = "    ".to_string();

    for (idx, line) in lines.iter().enumerate().take(c_end).skip(c_start + 1) {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with('*') {
            continue;
        }
        if trimmed.contains('(') {
            continue;
        }
        let words: Vec<&str> = trimmed.split_whitespace().collect();
        let is_match = words.iter().any(|w| {
            let clean = w.trim_matches(|c| c == ':' || c == '=');
            clean == target_field
        });
        if is_match
            && (trimmed.starts_with("var ")
                || trimmed.starts_with("let ")
                || trimmed.starts_with("public var ")
                || trimmed.starts_with("public let "))
        {
            field_line_idx = Some(idx);
            let indent_len = line.len() - trimmed.len();
            field_indent = line[..indent_len].to_string();
            if let Some(colon_pos) = trimmed.find(':') {
                let after = &trimmed[colon_pos + 1..];
                let ty_end = after.find('=').unwrap_or(after.len());
                field_type = after[..ty_end].trim().to_string();
            }
            break;
        }
    }

    let f_idx = field_line_idx.with_context(|| {
        format!("Field `{target_field}` not found in Swift type `{class_name}`")
    })?;
    let pascal = to_pascal_case(target_field);
    let mut_kw = if is_struct { "mutating " } else { "" };
    let accessors = format!(
        "\n{field_indent}func get{pascal}() -> {field_type} {{\n{field_indent}    return _{target_field}\n{field_indent}}}\n\n{field_indent}{mut_kw}func set{pascal}(_ {target_field}: {field_type}) {{\n{field_indent}    _{target_field} = {target_field}\n{field_indent}}}\n"
    );

    let new_field_decl = format!("{field_indent}private var _{target_field}: {field_type}");

    let mut out_lines = Vec::new();
    let mut left_direct = 0;
    let mut reads = 0;
    let mut writes = 0;
    for (idx, line) in lines.iter().enumerate() {
        if idx == f_idx {
            out_lines.push(new_field_decl.clone());
        } else if idx > c_start && idx < c_end {
            let (replaced, cnt) = replace_line_self(line, target_field);
            left_direct += cnt;
            let (replaced, r, w) = rewrite_external_swift(&replaced, target_field);
            reads += r;
            writes += w;
            out_lines.push(replaced);
        } else if idx == c_end {
            out_lines.push(accessors.clone());
            out_lines.push(line.to_string());
        } else {
            let (replaced, r, w) = rewrite_external_swift(line, target_field);
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
