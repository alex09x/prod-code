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

/// Core transformation for C++.
pub fn transform_cpp(
    text: &str,
    block: &ConditionalBlock,
    base_name: &str,
    method_name: &str,
    params: &[String],
    return_type: Option<&str>,
    target_var: &str,
) -> Result<String> {
    let mut out = text.to_string();
    let indent = &block.indent;
    let returns_value = returns_from_conditional(block)?;

    let ret_type = return_type.unwrap_or("void");
    anyhow::ensure!(
        returns_value || ret_type == "void",
        "a statement conditional cannot be assigned a non-void polymorphic return type"
    );
    let params_str = params.join(", ");
    let call_args = params
        .iter()
        .map(|p| p.split_whitespace().last().unwrap_or(p).trim())
        .collect::<Vec<_>>()
        .join(", ");

    let mut generated = Vec::new();

    // 1. Abstract base class
    let base_cls = format!(
        "class {base_name} {{\npublic:\n    virtual ~{base_name}() = default;\n    virtual {ret_type} {method_name}({params_str}) = 0;\n}};\n"
    );
    generated.push(base_cls);

    // 2. Concrete classes
    for b in &block.branches {
        if b.is_default && b.body.trim().contains("throw") {
            continue;
        }
        let v_name = format!("{}{base_name}", b.variant_name);
        let indented_body = b
            .body
            .lines()
            .map(|l| format!("        {}", l.trim()))
            .collect::<Vec<_>>()
            .join("\n");
        let cls = format!(
            "class {v_name} : public {base_name} {{\npublic:\n    {ret_type} {method_name}({params_str}) override {{\n{indented_body}\n    }}\n}};\n"
        );
        generated.push(cls);
    }

    // 3. Replacement
    let arrow_or_dot = if target_var.contains('*') || target_var.contains("ptr") {
        "->"
    } else {
        "."
    };
    let return_prefix = if returns_value { "return " } else { "" };
    let replacement =
        format!("{indent}{return_prefix}{target_var}{arrow_or_dot}{method_name}({call_args});");
    out.replace_range(block.start_offset..block.end_offset, &replacement);

    // 4. Prepend classes
    let decls = format!("{}\n\n", generated.join("\n"));
    out.insert_str(0, &decls);

    Ok(out)
}
