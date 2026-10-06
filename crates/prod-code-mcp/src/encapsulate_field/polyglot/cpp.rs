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

use super::super::case::to_snake_case;
use super::replace::replace_cpp_unqualified;

pub fn rewrite_external_cpp(code: &str, field: &str) -> (String, usize, usize) {
    let snake = to_snake_case(field);
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
        let dot_needle = format!(".{field}");
        let arrow_needle = format!("->{field}");
        if !line.contains(&dot_needle) && !line.contains(&arrow_needle) {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let mut new_line = String::new();
        let mut rest = line;
        while let Some(pos) = rest.find(&dot_needle).or_else(|| rest.find(&arrow_needle)) {
            let is_arrow = rest[pos..].starts_with("->");
            let op_len = if is_arrow { 2 } else { 1 };
            let total_len = op_len + field.len();
            let before = &rest[..pos];
            let after = &rest[pos + total_len..];
            let before_trimmed = before.trim_end();
            if before_trimmed.ends_with("this") {
                new_line.push_str(&rest[..pos + total_len]);
                rest = after;
                continue;
            }
            let trimmed_after = after.trim_start();
            if trimmed_after.starts_with('(') {
                new_line.push_str(&rest[..pos + total_len]);
                rest = after;
                continue;
            }
            if let Some(c) = after.chars().next()
                && (c.is_alphanumeric() || c == '_')
            {
                new_line.push_str(&rest[..pos + total_len]);
                rest = after;
                continue;
            }
            let op_str = if is_arrow { "->" } else { "." };
            if trimmed_after.starts_with('=') && !trimmed_after.starts_with("==") {
                writes += 1;
                let rhs_with_sep = trimmed_after[1..].trim_start();
                let (rhs, sep) = if let Some(semi_pos) = rhs_with_sep.find(';') {
                    (&rhs_with_sep[..semi_pos], &rhs_with_sep[semi_pos..])
                } else {
                    (rhs_with_sep, "")
                };
                new_line.push_str(before);
                new_line.push_str(&format!("{op_str}set_{snake}({rhs}){sep}"));
                rest = "";
                break;
            } else {
                reads += 1;
                new_line.push_str(before);
                new_line.push_str(&format!("{op_str}get_{snake}()"));
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

pub fn encapsulate_field_cpp(
    text: &str,
    target_class: Option<&str>,
    target_field: &str,
    by_value: Option<bool>,
) -> Result<(String, String, String, usize, usize, usize)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut class_start = None;
    let mut class_end = None;
    let mut class_name = String::new();
    let mut brace_depth = 0i32;

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("class ") || trimmed.starts_with("struct ") {
            let words: Vec<&str> = trimmed.split_whitespace().collect();
            let mut name = "";
            for (w_idx, w) in words.iter().enumerate() {
                if (*w == "class" || *w == "struct") && w_idx + 1 < words.len() {
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
            brace_depth += line.chars().filter(|&c| c == '{').count() as i32;
            brace_depth -= line.chars().filter(|&c| c == '}').count() as i32;
            if brace_depth == 0 && line.contains('}') {
                class_end = Some(idx);
                break;
            }
        }
    }

    let c_start = class_start.context("Could not find class or struct in C++ file")?;
    let c_end = class_end.context("Could not find closing brace of C++ class")?;

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
            let clean = w.trim_matches(|c| c == ';' || c == '=');
            clean == target_field
        });
        if is_match {
            field_line_idx = Some(idx);
            let indent_len = line.len() - trimmed.len();
            field_indent = line[..indent_len].to_string();
            if let Some(pos) = line.find(target_field) {
                let ty_part = line[..pos].trim_start();
                let clean_ty = ty_part
                    .trim_start_matches("public:")
                    .trim_start_matches("private:")
                    .trim_start_matches("protected:")
                    .trim();
                field_type = clean_ty.to_string();
            }
            break;
        }
    }

    let f_idx = field_line_idx
        .with_context(|| format!("Field `{target_field}` not found in class `{class_name}`"))?;
    let is_primitive = matches!(
        field_type.as_str(),
        "int"
            | "long"
            | "short"
            | "float"
            | "double"
            | "bool"
            | "char"
            | "size_t"
            | "int32_t"
            | "int64_t"
            | "uint32_t"
            | "uint64_t"
    );
    let ret_by_val = by_value.unwrap_or(is_primitive);
    let ret_ty = if ret_by_val {
        field_type.clone()
    } else {
        format!("const {}&", field_type)
    };
    let param_ty = if ret_by_val {
        field_type.clone()
    } else {
        format!("const {}&", field_type)
    };
    let snake = to_snake_case(target_field);

    let accessors = format!(
        "\npublic:\n{field_indent}{ret_ty} get_{snake}() const {{\n{field_indent}    return {target_field}_;\n{field_indent}}}\n\n{field_indent}void set_{snake}({param_ty} {target_field}) {{\n{field_indent}    {target_field}_ = {target_field};\n{field_indent}}}\n\nprivate:\n{field_indent}{field_type} {target_field}_;\n"
    );

    let mut out_lines = Vec::new();
    let mut left_direct = 0;
    let mut in_block_comment = false;
    let mut reads = 0;
    let mut writes = 0;
    for (idx, line) in lines.iter().enumerate() {
        if idx == f_idx {
            continue;
        } else if idx > c_start && idx < c_end {
            let (replaced, bare_count, shadowed) =
                replace_cpp_unqualified(line, target_field, &mut in_block_comment);
            anyhow::ensure!(
                !shadowed,
                "cannot safely rewrite unqualified `{target_field}` uses because a local or parameter with that name may shadow the field"
            );
            left_direct += bare_count;
            let (replaced, r, w) = rewrite_external_cpp(&replaced, target_field);
            reads += r;
            writes += w;
            out_lines.push(replaced);
        } else if idx == c_end {
            out_lines.push(accessors.clone());
            out_lines.push(line.to_string());
        } else {
            let (replaced, r, w) = rewrite_external_cpp(line, target_field);
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
