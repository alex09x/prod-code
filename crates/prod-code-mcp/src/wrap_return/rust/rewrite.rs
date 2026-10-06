/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::parameter_object::Language;
use crate::wrap_return::utils::{format_constructor_call, is_ident};

/// Rewrites explicit return statements and the trailing expression in a Rust function body.
pub(crate) fn rewrite_rust_body(
    body: &str,
    constructor: Option<&str>,
    envelope_base: &str,
    was: &str,
) -> String {
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    let bytes = body.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        match bytes[i] {
            b'r' if body[i..].starts_with("return") => {
                let before = if i > 0 { bytes[i - 1] as char } else { ' ' };
                let after = if i + 6 < bytes.len() {
                    bytes[i + 6] as char
                } else {
                    ' '
                };
                if !is_ident(before) && !is_ident(after) {
                    let end_stmt = body[i..].find(';').map_or(body.len(), |e| i + e);
                    let ret_stmt = &body[i..end_stmt];
                    let expr = ret_stmt.strip_prefix("return").unwrap().trim();
                    let wrapped = format_constructor_call(
                        constructor,
                        envelope_base,
                        expr,
                        Language::Rust,
                        was,
                    );
                    edits.push((i, end_stmt - i, format!("return {wrapped}")));
                    i = end_stmt;
                    continue;
                }
            }
            _ => {}
        }
        i += 1;
    }

    // Check if the body has a tail expression (not terminated by semicolon)
    let trimmed_body = body.trim_end();
    if !trimmed_body.is_empty() && !trimmed_body.ends_with(';') {
        let mut last_boundary = 0;
        let mut d = 0;
        let end_idx = trimmed_body.len();
        let mut in_str = false;
        let mut in_line_comment = false;
        let mut in_block_comment = false;
        let mut escape = false;

        for (idx, &b) in bytes.iter().enumerate().take(end_idx) {
            if in_line_comment {
                if b == b'\n' {
                    in_line_comment = false;
                }
                continue;
            }
            if in_block_comment {
                if b == b'/' && idx > 0 && bytes[idx - 1] == b'*' {
                    in_block_comment = false;
                }
                continue;
            }
            if in_str {
                if escape {
                    escape = false;
                } else if b == b'\\' {
                    escape = true;
                } else if b == b'"' {
                    in_str = false;
                }
                continue;
            }
            if b == b'"' {
                in_str = true;
                continue;
            }
            if b == b'/' && idx + 1 < end_idx {
                if bytes[idx + 1] == b'/' {
                    in_line_comment = true;
                    continue;
                } else if bytes[idx + 1] == b'*' {
                    in_block_comment = true;
                    continue;
                }
            }
            match b {
                b'{' => d += 1,
                b'}' => {
                    d -= 1;
                    if d == 0 && idx + 1 < end_idx {
                        last_boundary = idx + 1;
                    }
                }
                b';' if d == 0 => {
                    last_boundary = idx + 1;
                }
                _ => {}
            }
        }
        let tail_slice = &body[last_boundary..end_idx];
        let tail_trimmed = tail_slice.trim();
        if !tail_trimmed.is_empty() && !tail_trimmed.starts_with("return") {
            let lead_ws = tail_slice.len() - tail_slice.trim_start().len();
            let tail_start = last_boundary + lead_ws;
            let tail_end = tail_start + tail_trimmed.len();
            let wrapped = format_constructor_call(
                constructor,
                envelope_base,
                tail_trimmed,
                Language::Rust,
                was,
            );
            edits.push((tail_start, tail_end - tail_start, wrapped));
        }
    }

    let mut out = body.to_string();
    edits.sort_by_key(|e| std::cmp::Reverse(e.0));
    for (start, len, repl) in edits {
        out.replace_range(start..start + len, &repl);
    }
    out
}
