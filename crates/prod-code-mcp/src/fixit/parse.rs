/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::{Edit, Fix};

fn machine_applicable(span: &serde_json::Value) -> Option<Edit> {
    if span.get("suggestion_applicability")?.as_str()? != "MachineApplicable" {
        return None;
    }
    Some(Edit {
        file: span.get("file_name")?.as_str()?.to_string(),
        start: span.get("byte_start")?.as_u64()? as usize,
        end: span.get("byte_end")?.as_u64()? as usize,
        line: span.get("line_start")?.as_u64()?,
        line_text: span
            .pointer("/text/0/text")
            .and_then(|t| t.as_str())
            .map(str::to_string),
        replacement: span.get("suggested_replacement")?.as_str()?.to_string(),
    })
}

/// The machine-applicable fixes in one `cargo --message-format=json` line: one per suggestion
/// (a child of the diagnostic), with all of its parts.
pub fn parse_fixes(line: &str) -> Vec<Fix> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
        return Vec::new();
    };
    if value.get("reason").and_then(|r| r.as_str()) != Some("compiler-message") {
        return Vec::new();
    }
    let Some(message) = value.get("message") else {
        return Vec::new();
    };
    let level = message
        .get("level")
        .and_then(|l| l.as_str())
        .unwrap_or("")
        .to_string();
    let code = message
        .pointer("/code/code")
        .and_then(|c| c.as_str())
        .map(str::to_string);
    let text = message
        .get("message")
        .and_then(|m| m.as_str())
        .unwrap_or("")
        .to_string();
    let mut out = Vec::new();
    let mut take = |node: &serde_json::Value, own: Option<&str>| {
        let edits: Vec<Edit> = node
            .get("spans")
            .and_then(|s| s.as_array())
            .into_iter()
            .flatten()
            .filter_map(machine_applicable)
            .collect();
        if !edits.is_empty() {
            out.push(Fix {
                level: level.clone(),
                code: code.clone(),
                message: match own {
                    Some(own) if !own.is_empty() && own != text => format!("{text}: {own}"),
                    _ => text.clone(),
                },
                edits,
            });
        }
    };
    take(message, None);
    for child in message
        .get("children")
        .and_then(|c| c.as_array())
        .into_iter()
        .flatten()
    {
        take(child, child.get("message").and_then(|m| m.as_str()));
    }
    out
}
