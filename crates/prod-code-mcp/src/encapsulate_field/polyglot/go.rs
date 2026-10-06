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

use super::super::case::{lowercase_first, to_pascal_case};

pub fn rewrite_external_go(code: &str, field: &str) -> (String, usize, usize) {
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
        let needle_pascal = format!(".{pascal}");
        let needle_orig = format!(".{field}");
        let needle = if line.contains(&needle_pascal) {
            needle_pascal
        } else if line.contains(&needle_orig) {
            needle_orig
        } else {
            out.push_str(line);
            out.push('\n');
            continue;
        };
        let mut new_line = String::new();
        let mut rest = line;
        while let Some(pos) = rest.find(&needle) {
            let before = &rest[..pos];
            let after = &rest[pos + needle.len()..];
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
                let rhs = trimmed_after[1..].trim();
                new_line.push_str(before);
                new_line.push_str(&format!(".Set{pascal}({rhs})"));
                rest = "";
                break;
            } else {
                reads += 1;
                new_line.push_str(before);
                new_line.push_str(&format!(".{pascal}()"));
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

pub fn encapsulate_field_go(
    text: &str,
    target_class: Option<&str>,
    target_field: &str,
) -> Result<(String, String, String, usize, usize, usize)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut struct_start = None;
    let mut struct_end = None;
    let mut struct_name = String::new();
    let mut brace_depth = 0i32;

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("type ") && trimmed.contains(" struct") {
            let words: Vec<&str> = trimmed.split_whitespace().collect();
            if words.len() >= 2 {
                let name = words[1];
                if target_class.is_none() || target_class == Some(name) {
                    struct_start = Some(idx);
                    struct_name = name.to_string();
                    brace_depth = 0;
                }
            }
        }
        if struct_start.is_some() && struct_end.is_none() {
            brace_depth += line.chars().filter(|&c| c == '{').count() as i32;
            brace_depth -= line.chars().filter(|&c| c == '}').count() as i32;
            if brace_depth == 0 && line.contains('}') {
                struct_end = Some(idx);
                break;
            }
        }
    }

    let s_start = struct_start.context("Could not find struct in Go file")?;
    let s_end = struct_end.context("Could not find closing brace of Go struct")?;

    let mut field_line_idx = None;
    let mut field_type = String::new();
    let mut field_indent = "\t".to_string();

    let target_unexported = lowercase_first(target_field);
    let target_pascal = to_pascal_case(target_field);

    for (idx, line) in lines.iter().enumerate().take(s_end).skip(s_start + 1) {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with('*') {
            continue;
        }
        let words: Vec<&str> = trimmed.split_whitespace().collect();
        if !words.is_empty() {
            let fld = words[0];
            if fld == target_field || fld == target_pascal || fld == target_unexported {
                field_line_idx = Some(idx);
                let indent_len = line.len() - trimmed.len();
                field_indent = line[..indent_len].to_string();
                if words.len() >= 2 {
                    field_type = words[1].to_string();
                }
                break;
            }
        }
    }

    let f_idx = field_line_idx
        .with_context(|| format!("Field `{target_field}` not found in struct `{struct_name}`"))?;
    let declared_name = lines[f_idx].split_whitespace().next().unwrap_or_default();
    anyhow::ensure!(
        !declared_name.chars().next().is_some_and(char::is_uppercase),
        "cannot encapsulate exported Go field `{declared_name}` without changing its public API and serialization behavior"
    );
    anyhow::ensure!(
        !lines[f_idx].contains('`'),
        "cannot encapsulate tagged Go field `{declared_name}` without preserving reflection behavior"
    );
    let unexported_field = if target_pascal.chars().all(|c| c.is_ascii_uppercase()) {
        target_pascal.to_lowercase()
    } else {
        lowercase_first(&target_pascal)
    };
    let new_field_line = format!("{field_indent}{unexported_field} {field_type}");

    let recv = struct_name
        .chars()
        .next()
        .unwrap_or('s')
        .to_lowercase()
        .to_string();
    let accessors = format!(
        "\nfunc ({recv} *{struct_name}) {target_pascal}() {field_type} {{\n\treturn {recv}.{unexported_field}\n}}\n\nfunc ({recv} *{struct_name}) Set{target_pascal}({unexported_field} {field_type}) {{\n\t{recv}.{unexported_field} = {unexported_field}\n}}\n"
    );

    let mut out_lines = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if idx == f_idx {
            out_lines.push(new_field_line.clone());
        } else if idx == s_end {
            out_lines.push(line.to_string());
            out_lines.push(accessors.clone());
        } else {
            out_lines.push(line.to_string());
        }
    }

    let final_code = out_lines.join("\n");
    let reads = 0;
    let writes = 0;
    Ok((struct_name, field_type, final_code, reads, writes, 0))
}
