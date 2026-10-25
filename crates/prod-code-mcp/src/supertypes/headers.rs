/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::extract_trait::{generics::impl_body_open_in_code, lex::lexical_code};

/// Read the actual impl header containing an analyzer position, excluding its body.
/// Reuse the Rust lexical mask and balanced header delimiters used by trait extraction.
pub(super) fn implementation_header(text: &str, line: u32, col: u32) -> Option<String> {
    let mut start = 0;
    let selected = text.split_inclusive('\n').nth(line as usize)?;
    for part in text.split_inclusive('\n').take(line as usize) {
        start += part.len();
    }
    let mut units = 0;
    let byte = selected.char_indices().find_map(|(byte, c)| {
        let found = (units == col).then_some(byte);
        units += c.len_utf16() as u32;
        found
    })?;
    let at = start + byte;
    let code = lexical_code(text);
    if !code[at] {
        return None;
    }
    let ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || !b.is_ascii();
    for (begin, _) in text
        .rmatch_indices("impl")
        .filter(|(begin, _)| *begin <= at)
    {
        if !code[begin]
            // `r#impl` is an identifier, including when it names the implemented trait.
            || text[..begin].ends_with("r#")
            || begin
                .checked_sub(1)
                .is_some_and(|i| ident(text.as_bytes()[i]))
            || text.as_bytes().get(begin + 4).is_some_and(|b| ident(*b))
        {
            continue;
        }
        let Ok(end) = impl_body_open_in_code(text, begin, &code) else {
            continue;
        };
        if at < end {
            // A const argument can contain declarations and derive attributes of its
            // own. Those positions belong to the nested braced scope, not this impl.
            // Once the argument closes, the outer self type belongs to the header.
            let mut braces = 0;
            for (&byte, &is_code) in text.as_bytes()[begin..at].iter().zip(&code[begin..at]) {
                if is_code {
                    match byte {
                        b'{' => braces += 1,
                        b'}' => braces -= 1,
                        _ => {}
                    }
                }
            }
            if braces != 0 || matches!(text.as_bytes()[at], b'{' | b'}') {
                return None;
            }
            // Keep line endings until impl_trait masks comments: flattening here would
            // extend a line comment across the remaining header. Keep literal values too.
            return Some(text[begin..end].to_string());
        }
        // A body containing the position is a boundary, even inside an outer const
        // argument. A body closed before the position is not: its impl may be a
        // completed item nested inside the containing header's const argument.
        let mut depth = 0;
        let mut closed = false;
        for (&byte, &is_code) in text.as_bytes()[end..at].iter().zip(&code[end..at]) {
            if !is_code {
                continue;
            }
            match byte {
                b'{' => depth += 1,
                b'}' => depth -= 1,
                _ => continue,
            }
            if depth == 0 {
                closed = true;
                break;
            }
        }
        if !closed {
            return None;
        }
        // Earlier unrelated impls also end before `at`, so cannot pass `at < end`.
    }
    None
}
