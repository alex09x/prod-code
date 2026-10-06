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

/// Core transformation for Swift.
pub fn transform_swift(
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
    anyhow::ensure!(
        returns_value || return_type.is_none_or(|ty| ty == "Void" || ty == "void"),
        "a statement conditional cannot be assigned a non-void polymorphic return type"
    );

    let ret_type_clause = returns_value
        .then_some(return_type)
        .flatten()
        .map(|rt| format!(" -> {rt}"))
        .unwrap_or_default();
    let params_str = params.join(", ");
    let call_args = params
        .iter()
        .map(|p| {
            let name = p.split(':').next().unwrap_or(p).trim();
            format!("{name}: {name}")
        })
        .collect::<Vec<_>>()
        .join(", ");

    let mut generated = Vec::new();

    // 1. Protocol
    let proto = format!(
        "protocol {base_name} {{\n    func {method_name}({params_str}){ret_type_clause}\n}}\n"
    );
    generated.push(proto);

    // 2. Structs implementing protocol
    for b in &block.branches {
        if b.is_default && b.body.trim().contains("fatalError") {
            continue;
        }
        let v_name = format!("{}{base_name}", b.variant_name);
        let indented_body = b
            .body
            .lines()
            .map(|l| format!("        {}", l.trim()))
            .collect::<Vec<_>>()
            .join("\n");
        let s = format!(
            "struct {v_name}: {base_name} {{\n    func {method_name}({params_str}){ret_type_clause} {{\n{indented_body}\n    }}\n}}\n"
        );
        generated.push(s);
    }

    // 3. Replacement
    let return_prefix = if returns_value { "return " } else { "" };
    let replacement = format!("{indent}{return_prefix}{target_var}.{method_name}({call_args})");
    out.replace_range(block.start_offset..block.end_offset, &replacement);

    // 4. Prepend protocol and structs
    let decls = format!("{}\n\n", generated.join("\n"));
    out.insert_str(0, &decls);

    Ok(out)
}
