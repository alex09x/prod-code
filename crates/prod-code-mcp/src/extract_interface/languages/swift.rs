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

pub fn extract_interface_swift(
    text: &str,
    symbol: &str,
    interface_name: &str,
    target_methods: &[String],
) -> Result<(String, Vec<String>)> {
    let class_marker = format!("class {symbol}");
    let struct_marker = format!("struct {symbol}");
    let type_pos = text
        .find(&class_marker)
        .or_else(|| text.find(&struct_marker))
        .with_context(|| format!("class/struct `{symbol}` not found in Swift source"))?;

    let open_brace = text[type_pos..]
        .find('{')
        .map(|idx| type_pos + idx)
        .context("type body opening brace not found")?;

    let close_brace = crate::pull_push::find_matching_brace(text, open_brace)
        .context("type body matching closing brace not found")?;

    let inner = &text[open_brace + 1..close_brace];
    let mut extracted_methods = Vec::new();
    let mut protocol_sigs = Vec::new();

    for line in inner.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("func ") && !trimmed.starts_with("mutating func ") {
            continue;
        }

        let after_func = if let Some(rest) = trimmed.strip_prefix("mutating func ") {
            rest
        } else if let Some(rest) = trimmed.strip_prefix("func ") {
            rest
        } else {
            trimmed
        };

        let Some(open_paren) = after_func.find('(') else {
            continue;
        };
        let method_name = after_func[..open_paren].trim();

        if !target_methods.is_empty() && !target_methods.iter().any(|m| m == method_name) {
            continue;
        }

        let sig = trimmed.split('{').next().unwrap_or(trimmed).trim();
        protocol_sigs.push(format!("    {sig}"));
        extracted_methods.push(method_name.to_string());
    }

    if extracted_methods.is_empty() {
        bail!("no matching methods found in `{symbol}` to extract");
    }

    let protocol_def = format!(
        "protocol {interface_name} {{\n{}\n}}\n\n",
        protocol_sigs.join("\n")
    );

    let mut out = text.to_string();
    let header = &text[type_pos..open_brace];
    let header_trimmed = header.trim_end();
    let where_at = swift_where_clause_start(header_trimmed);
    let (inheritance_header, where_clause) = where_at.map_or((header_trimmed, ""), |at| {
        (header_trimmed[..at].trim_end(), &header_trimmed[at..])
    });
    let new_header = if let Some(separator) = swift_inheritance_separator(inheritance_header) {
        let existing = split_swift_inheritance(&inheritance_header[separator + 1..]);
        let mut bases = Vec::new();
        let is_class = inheritance_header
            .split_whitespace()
            .any(|token| token == "class");
        if is_class && !existing.is_empty() {
            bases.push(existing[0].clone());
            bases.push(interface_name.to_string());
            bases.extend(existing.into_iter().skip(1));
        } else {
            bases.extend(existing);
            bases.push(interface_name.to_string());
        }
        format!(
            "{}: {}{}{} ",
            inheritance_header[..separator].trim_end(),
            bases.join(", "),
            if where_clause.is_empty() { "" } else { " " },
            where_clause
        )
    } else {
        format!(
            "{inheritance_header}: {interface_name}{}{} ",
            if where_clause.is_empty() { "" } else { " " },
            where_clause
        )
    };
    out.replace_range(type_pos..open_brace, &new_header);

    let type_line_start = text[..type_pos].rfind('\n').map_or(0, |i| i + 1);
    out.insert_str(type_line_start, &protocol_def);

    Ok((out, extracted_methods))
}

pub(crate) fn swift_inheritance_separator(header: &str) -> Option<usize> {
    let mut angle_depth = 0usize;
    for (i, ch) in header.char_indices() {
        match ch {
            '<' => angle_depth += 1,
            '>' => angle_depth = angle_depth.saturating_sub(1),
            ':' if angle_depth == 0 => return Some(i),
            _ => {}
        }
    }
    None
}

pub(crate) fn swift_where_clause_start(header: &str) -> Option<usize> {
    let mut angle_depth = 0usize;
    for (i, ch) in header.char_indices() {
        match ch {
            '<' => angle_depth += 1,
            '>' => angle_depth = angle_depth.saturating_sub(1),
            'w' if angle_depth == 0 && header[i..].starts_with("where ") => {
                let before_is_boundary = i == 0
                    || header[..i]
                        .chars()
                        .next_back()
                        .is_some_and(|c| c.is_whitespace());
                if before_is_boundary {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

pub(crate) fn split_swift_inheritance(inheritance: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut angle_depth = 0usize;
    let mut start = 0usize;
    for (i, ch) in inheritance.char_indices() {
        match ch {
            '<' => angle_depth += 1,
            '>' => angle_depth = angle_depth.saturating_sub(1),
            ',' if angle_depth == 0 => {
                let part = inheritance[start..i].trim();
                if !part.is_empty() {
                    parts.push(part.to_string());
                }
                start = i + ch.len_utf8();
            }
            _ => {}
        }
    }
    let part = inheritance[start..].trim();
    if !part.is_empty() {
        parts.push(part.to_string());
    }
    parts
}
