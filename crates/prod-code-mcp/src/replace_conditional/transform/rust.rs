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

use super::super::types::ConditionalBlock;

/// Core transformation for Rust.
pub fn transform_rust(
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

    let ret_type_clause = return_type
        .map(|rt| format!(" -> {rt}"))
        .unwrap_or_default();
    let params_str = if params.is_empty() {
        "&self".to_string()
    } else {
        format!("&self, {}", params.join(", "))
    };
    let call_args = params
        .iter()
        .map(|p| p.split(':').next().unwrap_or(p).trim())
        .collect::<Vec<_>>()
        .join(", ");

    let mut generated = Vec::new();

    // 1. Trait
    let trt = format!(
        "pub trait {base_name} {{\n    fn {method_name}({params_str}){ret_type_clause};\n}}\n"
    );
    generated.push(trt);

    // 2. Structs + Impl
    for b in &block.branches {
        if b.is_default && b.body.trim().contains("panic") {
            continue;
        }
        let v_name = format!("{}{base_name}", b.variant_name);
        let indented_body = b
            .body
            .lines()
            .map(|l| format!("        {}", l.trim()))
            .collect::<Vec<_>>()
            .join("\n");
        let item = format!(
            "pub struct {v_name};\nimpl {base_name} for {v_name} {{\n    fn {method_name}({params_str}){ret_type_clause} {{\n{indented_body}\n    }}\n}}\n"
        );
        generated.push(item);
    }

    // 3. Replacement
    let replacement = format!("{indent}{target_var}.{method_name}({call_args})");
    out.replace_range(block.start_offset..block.end_offset, &replacement);

    // 4. Prepend trait + impls
    let decls = format!("{}\n\n", generated.join("\n"));
    out.insert_str(0, &decls);

    Ok(out)
}
