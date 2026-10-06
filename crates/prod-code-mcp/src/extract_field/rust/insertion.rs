/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

/// The edit that declares `decl` as the last field of the struct whose braces are `open..close`.
pub fn field_insertion(
    text: &str,
    open: usize,
    close: usize,
    decl: &str,
) -> (usize, usize, String) {
    let content_end = text[..close].trim_end().len();
    let first_field = text[open + 1..close]
        .lines()
        .find(|l| !l.trim().is_empty() && !l.trim_start().starts_with("//"));
    let indent: String = first_field
        .map(|l| l.chars().take_while(|c| c.is_whitespace()).collect())
        .unwrap_or_else(|| "    ".to_string());
    let comma = if content_end > open + 1 && !text[..content_end].ends_with(',') {
        ","
    } else {
        ""
    };
    let line_start = text[..close].rfind('\n').map_or(0, |i| i + 1);
    let closing_indent = &text[line_start..close];
    let closing_indent = if closing_indent.trim().is_empty() {
        closing_indent
    } else {
        ""
    };
    (
        content_end,
        close - content_end,
        format!("{comma}\n{indent}{decl},\n{closing_indent}"),
    )
}

/// The edit that initialises `field` first in the literal whose braces open at `open`.
pub fn literal_insertion(text: &str, open: usize, field_init: &str) -> (usize, usize, String) {
    let rest = &text[open + 1..];
    let same_line = rest.split('\n').next().unwrap_or("");
    if same_line.trim().is_empty() {
        // One field per line: the new one goes on its own line, indented like the next.
        let next = rest
            .lines()
            .skip(1)
            .find(|l| !l.trim().is_empty())
            .unwrap_or("");
        let indent: String = next.chars().take_while(|c| c.is_whitespace()).collect();
        return (open + 1, 0, format!("\n{indent}{field_init},"));
    }
    if rest.trim_start().starts_with('}') {
        return (open + 1, 0, format!(" {field_init} "));
    }
    (open + 1, 0, format!(" {field_init},"))
}
