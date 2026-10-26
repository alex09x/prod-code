/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

/// A byte-aligned return-type view. Attributes belong to the declaration, not
/// the type being wrapped. Mask the entire balanced attribute, including its
/// punctuation; lexical masking alone only hides its strings and comments.
pub(crate) fn type_code(text: &str) -> String {
    let mut clean = super::cpp_boundary::code(text).into_bytes();
    let spans = super::cpp_signature::token_spans(text);
    let mut i = 0;
    while i < spans.len() {
        let (start, end) = spans[i];
        let token = &text[start..end];
        if token != "[" || spans.get(i + 1).is_none_or(|&(s, e)| &text[s..e] != "[") {
            // Type-level literals (e.g. decltype(sizeof("x"))) are part of the
            // replaced type, unlike literal arguments inside an attribute.
            let line = text[..start].rfind('\n').map_or(0, |at| at + 1);
            if literal(token) && !text[line..].trim_start().starts_with('#') {
                clean[start..end].copy_from_slice(&text.as_bytes()[start..end]);
            }
            i += 1;
            continue;
        }
        let mut depth = 2;
        i += 2;
        let mut end = text.len();
        while i < spans.len() && depth > 0 {
            let (s, e) = spans[i];
            match &text[s..e] {
                "[" => depth += 1,
                "]" => depth -= 1,
                _ => {}
            }
            i += 1;
            if depth == 0 {
                end = e;
            }
        }
        // An incomplete attribute leaves no trustworthy return-type suffix.
        clean[start..end].fill(b' ');
    }
    String::from_utf8(clean).expect("attribute masking preserves UTF-8 code")
}

pub(super) fn literal(token: &str) -> bool {
    token.starts_with(['"', '\''])
        || ["R\"", "u8R\"", "uR\"", "UR\"", "LR\""]
            .iter()
            .any(|prefix| token.starts_with(prefix))
}

/// Structural boundaries must also ignore punctuation inside type literals.
pub(super) fn declaration_code(text: &str) -> String {
    let mut clean = type_code(text).into_bytes();
    for (start, end) in super::cpp_signature::token_spans(text) {
        if literal(&text[start..end]) {
            clean[start..end].fill(b' ');
        }
    }
    String::from_utf8(clean).expect("literal masking preserves UTF-8 code")
}
