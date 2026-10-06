/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

/// Find matching closing brace `}` for `{` at `open_idx`, ignoring strings and comments.
pub fn find_matching_brace(text: &str, open_idx: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    if bytes.get(open_idx) != Some(&b'{') {
        return None;
    }
    let mut depth = 0usize;
    let mut i = open_idx;
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut in_backtick = false;

    while i < bytes.len() {
        let b = bytes[i];
        let next = if i + 1 < bytes.len() { bytes[i + 1] } else { 0 };

        if in_line_comment {
            if b == b'\n' {
                in_line_comment = false;
            }
            i += 1;
            continue;
        }
        if in_block_comment {
            if b == b'*' && next == b'/' {
                in_block_comment = false;
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        if in_single_quote {
            if b == b'\\' {
                i += 2;
                continue;
            }
            if b == b'\'' {
                in_single_quote = false;
            }
            i += 1;
            continue;
        }
        if in_double_quote {
            if b == b'\\' {
                i += 2;
                continue;
            }
            if b == b'"' {
                in_double_quote = false;
            }
            i += 1;
            continue;
        }
        if in_backtick {
            if b == b'\\' {
                i += 2;
                continue;
            }
            if b == b'`' {
                in_backtick = false;
            }
            i += 1;
            continue;
        }

        if b == b'/' && next == b'/' {
            in_line_comment = true;
            i += 2;
            continue;
        }
        if b == b'/' && next == b'*' {
            in_block_comment = true;
            i += 2;
            continue;
        }

        if b == b'\'' {
            in_single_quote = true;
            i += 1;
            continue;
        }
        if b == b'"' {
            in_double_quote = true;
            i += 1;
            continue;
        }
        if b == b'`' {
            in_backtick = true;
            i += 1;
            continue;
        }

        if b == b'{' {
            depth += 1;
        } else if b == b'}' {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}
