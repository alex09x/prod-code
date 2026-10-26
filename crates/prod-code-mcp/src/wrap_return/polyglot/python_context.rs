/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::{caller_migration::mask::lexical_code_mask, parameter_object::Language};

/// Import context ends with the logical statement, including on a semicolon line.
/// Keep offsets and bracket continuations; delimiters in strings/comments are opaque.
pub(crate) fn is_import(text: &str, at: usize) -> bool {
    let mask = lexical_code_mask(text, Language::Python);
    let bytes = text.as_bytes();
    let mut start = 0;
    let mut depth = 0usize;
    for i in 0..at {
        if !mask[i] {
            continue;
        }
        match bytes[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b';' if depth == 0 => start = i + 1,
            b'\n' if depth == 0 => {
                let previous = if i > 0 && bytes[i - 1] == b'\r' {
                    i - 1
                } else {
                    i
                };
                let continued = previous > 0 && bytes[previous - 1] == b'\\' && mask[previous - 1];
                if !continued {
                    start = i + 1;
                }
            }
            _ => {}
        }
    }
    matches!(
        text[start..at].split_whitespace().next(),
        Some("from" | "import")
    )
}
