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
use super::replace::replace_line_self;

pub fn rewrite_external_py(code: &str, field: &str) -> (String, usize, usize) {
    let snake = to_snake_case(field);
    let mut reads = 0;
    let mut writes = 0;
    let mut out = String::new();
    for line in code.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
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
                let rhs = trimmed_after[1..].trim();
                new_line.push_str(before);
                new_line.push_str(&format!(".set_{snake}({rhs})"));
                rest = "";
                break;
            } else {
                reads += 1;
                new_line.push_str(before);
                new_line.push_str(&format!(".get_{snake}()"));
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

pub fn encapsulate_field_py(
    text: &str,
    target_class: Option<&str>,
    target_field: &str,
) -> Result<(String, String, String, usize, usize, usize)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut class_start = None;
    let mut class_name = String::new();
    let mut class_end = lines.len();

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("class ") {
            let name = rest.split(['(', ':']).next().unwrap_or("").trim();
            if target_class.is_none() || target_class == Some(name) {
                class_start = Some(idx);
                class_name = name.to_string();
                break;
            }
        }
    }

    let c_start = class_start.context("Could not find class in Python file")?;
    for (idx, line) in lines.iter().enumerate().skip(c_start + 1) {
        if !line.trim().is_empty() && !line.starts_with(' ') && !line.starts_with('\t') {
            class_end = idx;
            break;
        }
    }

    let mut field_type = String::new();
    let mut indent = "    ".to_string();
    let mut left_direct = 0;
    let mut reads = 0;
    let mut writes = 0;
    let mut out_lines = Vec::new();

    for (idx, line) in lines.iter().enumerate() {
        if idx > c_start && idx < class_end {
            let trimmed = line.trim_start();
            if trimmed.starts_with("def ") {
                let cur_indent_len = line.len() - trimmed.len();
                indent = line[..cur_indent_len].to_string();
            }
            let self_needle = format!("self.{target_field}");
            if line.contains(&self_needle) {
                let (replaced, cnt) = replace_line_self(line, target_field);
                left_direct += cnt;
                if trimmed.contains(&format!("self.{target_field}:"))
                    && let Some(pos) = trimmed.find(':')
                {
                    let after = &trimmed[pos + 1..];
                    let ty_end = after.find('=').unwrap_or(after.len());
                    field_type = after[..ty_end].trim().to_string();
                }
                let (replaced, r, w) = rewrite_external_py(&replaced, target_field);
                reads += r;
                writes += w;
                out_lines.push(replaced);
            } else if trimmed.starts_with(&format!("{target_field}:"))
                || trimmed.starts_with(&format!("{target_field} ="))
            {
                let cur_indent_len = line.len() - trimmed.len();
                let cur_indent = &line[..cur_indent_len];
                if let Some(pos) = trimmed.find(':') {
                    let after = &trimmed[pos + 1..];
                    let ty_end = after.find('=').unwrap_or(after.len());
                    field_type = after[..ty_end].trim().to_string();
                }
                let rest_of_line = &trimmed[target_field.len()..];
                out_lines.push(format!("{cur_indent}_{target_field}{rest_of_line}"));
                left_direct += 1;
            } else {
                let (replaced, r, w) = rewrite_external_py(line, target_field);
                reads += r;
                writes += w;
                out_lines.push(replaced);
            }
        } else {
            let (replaced, r, w) = rewrite_external_py(line, target_field);
            reads += r;
            writes += w;
            out_lines.push(replaced);
        }
    }

    if field_type.is_empty() {
        for line in &lines[c_start..class_end] {
            if let Some(pos) = line.find(&format!("{target_field}:")) {
                let after = &line[pos + target_field.len() + 1..];
                let ty_end = after
                    .find([',', ')', '=', '#', '\n'])
                    .unwrap_or(after.len());
                let found = after[..ty_end].trim();
                if !found.is_empty() {
                    field_type = found.to_string();
                    break;
                }
            }
        }
    }

    let snake = to_snake_case(target_field);
    let ret_annot = if field_type.is_empty() {
        String::new()
    } else {
        format!(" -> {field_type}")
    };
    let param_annot = if field_type.is_empty() {
        String::new()
    } else {
        format!(": {field_type}")
    };
    let accessors = format!(
        "\n{indent}def get_{snake}(self){ret_annot}:\n{indent}    return self._{target_field}\n\n{indent}def set_{snake}(self, {target_field}{param_annot}) -> None:\n{indent}    self._{target_field} = {target_field}\n"
    );

    out_lines.insert(class_end, accessors);

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
