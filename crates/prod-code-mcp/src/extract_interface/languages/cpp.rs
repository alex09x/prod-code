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

pub fn extract_interface_cpp(
    text: &str,
    symbol: &str,
    interface_name: &str,
    target_methods: &[String],
) -> Result<(String, Vec<String>)> {
    let class_marker = format!("class {symbol}");
    let struct_marker = format!("struct {symbol}");
    let class_pos = text
        .find(&class_marker)
        .or_else(|| text.find(&struct_marker))
        .with_context(|| format!("class/struct `{symbol}` not found in C++ source"))?;

    let open_brace = text[class_pos..]
        .find('{')
        .map(|idx| class_pos + idx)
        .context("class body opening brace not found")?;

    let close_brace = crate::pull_push::find_matching_brace(text, open_brace)
        .context("class body matching closing brace not found")?;

    let inner = &text[open_brace + 1..close_brace];
    let mut extracted_methods = Vec::new();
    let mut interface_sigs = Vec::new();

    for line in inner.lines() {
        let trimmed = line.trim();
        // Skip destructor, private markers, comments
        if trimmed.starts_with('~')
            || trimmed.starts_with("private:")
            || trimmed.starts_with("protected:")
            || trimmed.starts_with("//")
            || trimmed.is_empty()
        {
            continue;
        }

        if let Some(open_paren) = trimmed.find('(') {
            let before_paren = trimmed[..open_paren].trim();
            let words: Vec<&str> = before_paren.split_whitespace().collect();
            if words.len() < 2 {
                continue; // Skip constructor or single identifier
            }

            let method_name = words.last().unwrap();
            if !target_methods.is_empty() && !target_methods.iter().any(|m| m == method_name) {
                continue;
            }

            let close_paren = trimmed.find(')').context("malformed parameter list")?;
            let params = &trimmed[open_paren + 1..close_paren];
            let ret_type = words[..words.len() - 1]
                .iter()
                .filter(|w| **w != "virtual" && **w != "inline" && **w != "explicit")
                .copied()
                .collect::<Vec<_>>()
                .join(" ");

            let after_paren = trimmed[close_paren + 1..]
                .split('{')
                .next()
                .unwrap_or("")
                .trim();
            let const_qual = if after_paren.contains("const") {
                " const"
            } else {
                ""
            };

            let sig = format!("    virtual {ret_type} {method_name}({params}){const_qual} = 0;");
            interface_sigs.push(sig);
            extracted_methods.push(method_name.to_string());
        }
    }

    if extracted_methods.is_empty() {
        bail!("no matching member functions found in `{symbol}` to extract");
    }

    let interface_def = format!(
        "class {interface_name} {{\npublic:\n    virtual ~{interface_name}() = default;\n{}\n}};\n\n",
        interface_sigs.join("\n")
    );

    let mut out = text.to_string();
    let header = &text[class_pos..open_brace];
    let header_trimmed = header.trim_end();
    let new_header = if let Some(separator) = cpp_inheritance_separator(header_trimmed) {
        let existing_bases = header_trimmed[separator + 1..].trim();
        if existing_bases.is_empty() {
            format!(
                "{}: public {interface_name} ",
                header_trimmed[..separator].trim_end()
            )
        } else {
            format!(
                "{}: public {interface_name}, {existing_bases} ",
                header_trimmed[..separator].trim_end()
            )
        }
    } else {
        format!("{header_trimmed} : public {interface_name} ")
    };
    out.replace_range(class_pos..open_brace, &new_header);

    let class_line_start = text[..class_pos].rfind('\n').map_or(0, |i| i + 1);
    out.insert_str(class_line_start, &interface_def);

    Ok((out, extracted_methods))
}

pub(crate) fn cpp_inheritance_separator(header: &str) -> Option<usize> {
    let mut angle_depth = 0usize;
    let bytes = header.as_bytes();
    for (i, byte) in bytes.iter().enumerate() {
        match byte {
            b'<' => angle_depth += 1,
            b'>' => angle_depth = angle_depth.saturating_sub(1),
            b':' if angle_depth == 0
                && bytes.get(i.wrapping_sub(1)) != Some(&b':')
                && bytes.get(i + 1) != Some(&b':') =>
            {
                return Some(i);
            }
            _ => {}
        }
    }
    None
}
