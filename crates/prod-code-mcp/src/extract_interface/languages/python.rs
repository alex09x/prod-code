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

pub fn extract_interface_python(
    text: &str,
    symbol: &str,
    interface_name: &str,
    target_methods: &[String],
) -> Result<(String, Vec<String>)> {
    let class_marker = format!("class {symbol}");
    let class_pos = text
        .find(&class_marker)
        .with_context(|| format!("class `{symbol}` not found in Python source"))?;

    let colon_rel = text[class_pos..]
        .find(':')
        .context("class definition colon not found")?;
    let header_end = class_pos + colon_rel;

    let mut extracted_methods = Vec::new();
    let mut protocol_methods = Vec::new();

    // Iterate through lines following class definition
    let after_class = &text[header_end + 1..];
    let class_indent = text[..class_pos].lines().last().map_or(0, |l| {
        l.chars().take_while(|c| *c == ' ' || *c == '\t').count()
    });
    let member_indent = after_class
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.trim().starts_with('#'))
        .map(|line| line.chars().take_while(|c| *c == ' ' || *c == '\t').count())
        .find(|indent| *indent > class_indent)
        .context("class body has no indented members")?;

    for line in after_class.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let line_indent = line.chars().take_while(|c| *c == ' ' || *c == '\t').count();
        if line_indent <= class_indent && !trimmed.starts_with('#') {
            // Reached next top-level or sibling declaration
            break;
        }

        if line_indent == member_indent && trimmed.starts_with("def ") {
            let Some(open_paren) = trimmed.find('(') else {
                continue;
            };
            let method_name = trimmed[4..open_paren].trim();

            // Skip dunder methods
            if method_name.starts_with("__") && method_name.ends_with("__") {
                continue;
            }

            if !target_methods.is_empty() && !target_methods.iter().any(|m| m == method_name) {
                continue;
            }

            let sig = trimmed.strip_suffix(':').unwrap_or(trimmed).trim();
            protocol_methods.push(format!("    {sig}:\n        ..."));
            extracted_methods.push(method_name.to_string());
        }
    }

    if extracted_methods.is_empty() {
        bail!("no matching methods found in class `{symbol}` to extract");
    }

    let protocol_def = format!(
        "class {interface_name}(Protocol):\n{}\n\n\n",
        protocol_methods.join("\n\n")
    );

    let mut out = text.to_string();

    // Update class header to inherit from Protocol
    let old_header = &text[class_pos..header_end];
    let new_header = if old_header.contains('(') {
        old_header.replace('(', &format!("({interface_name}, "))
    } else {
        format!("{old_header}({interface_name})")
    };
    out.replace_range(class_pos..header_end, &new_header);

    // Prepend protocol definition
    let class_line_start = text[..class_pos].rfind('\n').map_or(0, |i| i + 1);
    out.insert_str(class_line_start, &protocol_def);

    // Ensure Protocol import is present
    if !out.contains("from typing import Protocol") && !out.contains("typing.Protocol") {
        out.insert_str(0, "from typing import Protocol\n\n");
    }

    Ok((out, extracted_methods))
}
