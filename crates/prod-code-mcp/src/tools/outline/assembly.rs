/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use serde_json::{Value, json};

/// Lexical labels only; preprocessing and assembler macros are not evaluated.
pub(super) fn symbols(source: &str) -> Value {
    let mut in_comment = false;
    let mut items = Vec::new();
    for (line_no, line) in source.lines().enumerate() {
        let mut visible = String::with_capacity(line.len());
        let bytes = line.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if in_comment {
                if bytes.get(i..i + 2) == Some(b"*/") {
                    in_comment = false;
                    i += 2;
                } else {
                    i += 1;
                }
            } else if bytes.get(i..i + 2) == Some(b"/*") {
                in_comment = true;
                i += 2;
            } else if bytes.get(i..i + 2) == Some(b"//") || bytes[i] == b'#' {
                break;
            } else {
                visible.push(bytes[i] as char);
                i += 1;
            }
        }
        let trimmed = visible.trim_start();
        let Some(colon) = trimmed.find(':') else {
            continue;
        };
        let name = &trimmed[..colon];
        if name.is_empty()
            || !name
                .bytes()
                .next()
                .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_' || b == b'.')
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'$'))
        {
            continue;
        }
        let column = line.find(name).unwrap_or(0);
        let start = line[..column].encode_utf16().count();
        let end = start + name.len();
        items.push(json!({"name": name, "kind": 13,
            "range": {"start": {"line": line_no, "character": start}, "end": {"line": line_no, "character": end}},
            "selectionRange": {"start": {"line": line_no, "character": start}, "end": {"line": line_no, "character": end}}}));
    }
    Value::Array(items)
}

#[cfg(test)]
mod tests {
    use super::symbols;

    #[test]
    fn labels_ignore_comments_directives_and_numeric_branches() {
        let source = "# preprocessor\n/* hidden:\nstill_hidden: */\n.globl entry\nentry: mov x0, x0\n.Lloop:\n1: nop\n// fake:\n";
        let result = symbols(source);
        let names: Vec<_> = result
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v["name"].as_str())
            .collect();
        assert_eq!(names, ["entry", ".Lloop"]);
        assert_eq!(result[0]["selectionRange"]["start"]["line"], 4);
    }
}
