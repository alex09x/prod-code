/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result, bail};

pub fn extract_interface_ts(
    text: &str,
    symbol: &str,
    interface_name: &str,
    target_methods: &[String],
) -> Result<(String, Vec<String>)> {
    let class_marker = format!("class {symbol}");
    let class_pos = text
        .find(&class_marker)
        .with_context(|| format!("class `{symbol}` not found in TypeScript source"))?;

    let open_brace = text[class_pos..]
        .find('{')
        .map(|idx| class_pos + idx)
        .context("class body opening brace not found")?;

    let close_brace = crate::pull_push::find_matching_brace(text, open_brace)
        .context("class body matching closing brace not found")?;

    let inner = &text[open_brace + 1..close_brace];
    let mut extracted_methods = Vec::new();
    let mut interface_sigs = Vec::new();
    let mut depth = 0;

    for line in inner.lines() {
        let trimmed = line.trim();
        // Skip constructors, private fields (#), and comments
        if trimmed.starts_with("constructor")
            || trimmed.starts_with("private ")
            || trimmed.starts_with('#')
            || trimmed.starts_with("//")
            || trimmed.starts_with("/*")
            || trimmed.is_empty()
        {
            for c in line.chars() {
                if c == '{' {
                    depth += 1;
                } else if c == '}' && depth > 0 {
                    depth -= 1;
                }
            }
            continue;
        }

        if depth == 0 {
            // Check if line contains a method signature: `methodName(...)`
            if let Some(open_paren) = trimmed.find('(') {
                let prefix = trimmed[..open_paren].trim();
                let method_name = prefix
                    .split_whitespace()
                    .last()
                    .unwrap_or(prefix)
                    .trim_start_matches("async ")
                    .trim_start_matches("public ");

                if !method_name.is_empty()
                    && method_name.chars().all(|c| c.is_alphanumeric() || c == '_')
                    && (target_methods.is_empty()
                        || target_methods.iter().any(|m| m == method_name))
                {
                    // Find closing paren and return type
                    if let Some(close_paren) = trimmed.find(')') {
                        let rest_after_paren = trimmed[close_paren + 1..].trim();
                        let ret_type = if let Some(stripped) = rest_after_paren.strip_prefix(':') {
                            stripped.split('{').next().unwrap_or(stripped).trim()
                        } else {
                            "any"
                        };

                        let params = &trimmed[open_paren + 1..close_paren];
                        let sig = format!("    {method_name}({params}): {ret_type};");
                        interface_sigs.push(sig);
                        extracted_methods.push(method_name.to_string());
                    }
                }
            }
        }

        for c in line.chars() {
            if c == '{' {
                depth += 1;
            } else if c == '}' && depth > 0 {
                depth -= 1;
            }
        }
    }

    if extracted_methods.is_empty() {
        bail!("no matching methods found in class `{symbol}` to extract");
    }

    let interface_def = format!(
        "export interface {interface_name} {{\n{}\n}}\n\n",
        interface_sigs.join("\n")
    );

    // Update class declaration with `implements {interface_name}`
    let mut out = text.to_string();
    let header = &text[class_pos..open_brace];
    let new_header = if header.contains("implements ") {
        header.replace("implements ", &format!("implements {interface_name}, "))
    } else {
        format!("{header}implements {interface_name} ")
    };

    out.replace_range(class_pos..open_brace, &new_header);

    // Find class line start (including export if present)
    let class_line_start = text[..class_pos].rfind('\n').map_or(0, |i| i + 1);
    out.insert_str(class_line_start, &interface_def);

    Ok((out, extracted_methods))
}
