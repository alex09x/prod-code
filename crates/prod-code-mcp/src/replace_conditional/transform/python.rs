/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::Result;

use super::super::types::{ConditionalBlock, returns_from_conditional};

/// Core transformation for Python.
pub fn transform_python(
    text: &str,
    block: &ConditionalBlock,
    base_name: &str,
    method_name: &str,
    params: &[String],
    target_var: &str,
) -> Result<String> {
    let mut out = text.to_string();
    let indent = &block.indent;
    let returns_value = returns_from_conditional(block)?;

    let params_str = if params.is_empty() {
        "self".to_string()
    } else {
        format!("self, {}", params.join(", "))
    };
    let call_args = if params.is_empty() {
        "".to_string()
    } else {
        params
            .iter()
            .map(|p| p.split(':').next().unwrap_or(p).trim())
            .collect::<Vec<_>>()
            .join(", ")
    };

    // 1. Generate base class
    let mut classes = Vec::new();
    let base_class = format!(
        "class {base_name}:\n    def {method_name}({params_str}):\n        raise NotImplementedError\n"
    );
    classes.push(base_class);

    // 2. Generate variant subclasses
    for b in &block.branches {
        if b.is_default && b.body.trim().contains("raise") {
            continue; // Skip default error branch from creating a class
        }
        let v_name = format!("{}{base_name}", b.variant_name);
        let indented_body = if b.body.trim().is_empty() {
            "        pass".to_string()
        } else {
            indent_python_branch_body(&b.body)
        };
        let cls = format!(
            "class {v_name}({base_name}):\n    def {method_name}({params_str}):\n{indented_body}\n"
        );
        classes.push(cls);
    }

    // 3. Replacement for conditional block
    let return_prefix = if returns_value { "return " } else { "" };
    let replacement = format!("{indent}{return_prefix}{target_var}.{method_name}({call_args})");
    out.replace_range(block.start_offset..block.end_offset, &replacement);

    // 4. Prepend classes right before enclosing function or at top of file
    let class_definitions = format!("{}\n\n", classes.join("\n"));
    out.insert_str(0, &class_definitions);

    Ok(out)
}

fn indent_python_branch_body(body: &str) -> String {
    let common_indent = body
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.chars().take_while(|c| *c == ' ' || *c == '\t').count())
        .min()
        .unwrap_or(0);
    body.lines()
        .map(|line| {
            if line.trim().is_empty() {
                "        ".to_string()
            } else {
                let cut = if common_indent == 0 {
                    0
                } else {
                    line.char_indices()
                        .take_while(|(_, c)| *c == ' ' || *c == '\t')
                        .nth(common_indent - 1)
                        .map_or(0, |(byte, c)| byte + c.len_utf8())
                };
                format!("        {}", &line[cut..])
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}
