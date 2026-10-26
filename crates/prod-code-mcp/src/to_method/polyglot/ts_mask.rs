/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::borrow::Cow;

/// Adapt LF-based scanners to ECMAScript's complete LineTerminator set. This
/// analysis-only view has exactly the original byte length: the two remaining
/// UTF-8 bytes of LS/PS become spaces. Never emit this view as source text.
pub(super) fn line_view(text: &str) -> Cow<'_, str> {
    if !text.contains(['\r', '\u{2028}', '\u{2029}']) {
        return Cow::Borrowed(text);
    }
    let mut view = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\r' => view.push('\n'),
            '\u{2028}' | '\u{2029}' => view.push_str("\n  "),
            ch => view.push(ch),
        }
    }
    Cow::Owned(view)
}

pub(super) fn executable_mask(text: &str) -> Vec<bool> {
    crate::caller_migration::lexical_code_mask(
        &line_view(text),
        crate::parameter_object::Language::TypeScript,
    )
}
