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

pub fn extract_interface_go(
    text: &str,
    symbol: &str,
    interface_name: &str,
    target_methods: &[String],
) -> Result<(String, Vec<String>)> {
    let type_marker = format!("type {symbol} ");
    let type_pos = text
        .find(&type_marker)
        .with_context(|| format!("type `{symbol}` not found in Go source"))?;

    let mut extracted_methods = Vec::new();
    let mut interface_sigs = Vec::new();

    // Look for methods with receiver `(r *{symbol})` or `(r {symbol})`
    for line in text.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("func ") {
            continue;
        }

        // Must have receiver
        let Some(open_paren) = trimmed.find('(') else {
            continue;
        };
        let Some(close_paren) = trimmed[open_paren..].find(')') else {
            continue;
        };
        let recv_part = &trimmed[open_paren + 1..open_paren + close_paren];

        // Check if receiver references symbol
        let recv_words: Vec<&str> = recv_part.split_whitespace().collect();
        let is_target_recv = recv_words.iter().any(|w| {
            let clean = w.trim_start_matches('*');
            clean == symbol
        });

        if !is_target_recv {
            continue;
        }

        // After receiver, extract method name, params, and return types
        let after_recv = trimmed[open_paren + close_paren + 1..].trim();
        let Some(sig_open_paren) = after_recv.find('(') else {
            continue;
        };
        let method_name = after_recv[..sig_open_paren].trim();

        if method_name.is_empty() {
            continue;
        }

        if !target_methods.is_empty() && !target_methods.iter().any(|m| m == method_name) {
            continue;
        }

        let sig_end = go_method_body_start(after_recv).unwrap_or(after_recv.len());
        let sig = after_recv[..sig_end].trim();
        interface_sigs.push(format!("\t{sig}"));
        extracted_methods.push(method_name.to_string());
    }

    if extracted_methods.is_empty() {
        bail!("no matching receiver methods found for type `{symbol}` to extract");
    }

    let interface_def = format!(
        "type {interface_name} interface {{\n{}\n}}\n\n",
        interface_sigs.join("\n")
    );

    let type_line_start = text[..type_pos].rfind('\n').map_or(0, |i| i + 1);
    let mut out = text.to_string();
    out.insert_str(type_line_start, &interface_def);

    Ok((out, extracted_methods))
}

pub(crate) fn go_method_body_start(signature: &str) -> Option<usize> {
    let bytes = signature.as_bytes();
    let mut i = 0;
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => paren_depth += 1,
            b')' => paren_depth = paren_depth.saturating_sub(1),
            b'[' => bracket_depth += 1,
            b']' => bracket_depth = bracket_depth.saturating_sub(1),
            b'{' => {
                let prefix = signature[..i].trim_end();
                let word_start = prefix
                    .rfind(|c: char| !(c.is_alphanumeric() || c == '_'))
                    .map_or(0, |pos| pos + 1);
                let preceding_word = &prefix[word_start..];
                if matches!(preceding_word, "interface" | "struct") {
                    let close = crate::parameter_object::matching_bracket(signature, i)?;
                    i = close + 1;
                    continue;
                }
                if paren_depth == 0 && bracket_depth == 0 {
                    return Some(i);
                }
                let close = crate::parameter_object::matching_bracket(signature, i)?;
                i = close + 1;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    None
}
