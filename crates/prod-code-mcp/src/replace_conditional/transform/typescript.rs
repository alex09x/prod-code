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

/// Core transformation for TypeScript / JavaScript.
pub fn transform_typescript(
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

    let ret_type = return_type.unwrap_or(if returns_value { "any" } else { "void" });
    anyhow::ensure!(
        returns_value || ret_type == "void",
        "a statement conditional cannot be assigned a non-void polymorphic return type"
    );
    let params_str = params.join(", ");
    let call_args = params
        .iter()
        .map(|p| p.split(':').next().unwrap_or(p).trim())
        .collect::<Vec<_>>()
        .join(", ");

    let mut generated = Vec::new();

    // 1. Generate interface
    let iface = format!(
        "export interface {base_name} {{\n    {method_name}({params_str}): {ret_type};\n}}\n"
    );
    generated.push(iface);

    // 2. Generate concrete classes
    for b in &block.branches {
        if b.is_default && b.body.trim().contains("throw") {
            continue;
        }
        let v_name = format!("{}{base_name}", b.variant_name);
        let indented_body = if b.body.trim().is_empty() {
            "        // default implementation".to_string()
        } else {
            b.body
                .lines()
                .map(|l| format!("        {}", l.trim()))
                .collect::<Vec<_>>()
                .join("\n")
        };
        let cls = format!(
            "export class {v_name} implements {base_name} {{\n    {method_name}({params_str}): {ret_type} {{\n{indented_body}\n    }}\n}}\n"
        );
        generated.push(cls);
    }

    // 3. Replacement
    let return_prefix = if returns_value { "return " } else { "" };
    let replacement = format!("{indent}{return_prefix}{target_var}.{method_name}({call_args});");
    out.replace_range(block.start_offset..block.end_offset, &replacement);

    // 4. Prepend declarations at top
    let decls = format!("{}\n\n", generated.join("\n"));
    out.insert_str(0, &decls);

    Ok(out)
}
