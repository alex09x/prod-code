/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::Token;
use std::ops::Range;

pub(super) struct Coverage {
    pub allowed: Vec<bool>,
    pub uncertain: Vec<Range<usize>>,
}

/// Do not discover token fragments inside grammar this expression parser does not model.
/// NEWLINE tokens already delimit logical statements, including joined physical lines.
/// Unknown executable statements must also be reported to replace-all callers.
pub(super) fn allowed(ts: &[Token<'_>], executable: &[bool]) -> Coverage {
    let mut allowed = executable.to_vec();
    let mut uncertain = Vec::new();
    let mut start = 0;
    for end in 0..=ts.len() {
        if end != ts.len() && !matches!(ts[end].text, "\n" | ";") {
            continue;
        }
        let statement = &ts[start..end];
        let mut nesting = 0usize;
        let keyword_arguments = statement.iter().any(|t| {
            match t.text {
                "(" | "[" => nesting += 1,
                ")" | "]" => nesting = nesting.saturating_sub(1),
                "=" if nesting > 0 => return true,
                _ => {}
            }
            false
        });
        let unsupported = keyword_arguments
            || statement.iter().any(|t| {
                matches!(
                    t.text,
                    "lambda"
                        | "for"
                        | "if"
                        | "else"
                        | "yield"
                        | "async"
                        | ":="
                        | "{"
                        | "}"
                        | "while"
                        | "with"
                        | "try"
                        | "except"
                        | "finally"
                        | "match"
                        | "case"
                        | ":"
                        | "del"
                        | "import"
                        | "from"
                )
            })
            || statement
                .iter()
                .any(|t| t.literal && super::super::literals::python_interpolated(t.text))
            || statement.windows(2).any(|w| {
                // String prefixes/interpolation and implicit concatenation have a
                // different expression extent from an isolated quoted token.
                let quoted = w[1].literal && w[1].text.contains(['\'', '"']);
                quoted
                    && (w[0].literal
                        || matches!(
                            w[0].text.to_ascii_lowercase().as_str(),
                            "r" | "b" | "u" | "f" | "br" | "rb" | "fr" | "rf"
                        ))
            });
        if unsupported {
            if executable[start..end].iter().any(|allowed| *allowed) {
                uncertain.push(start..end);
            }
            allowed[start..end].fill(false);
        }
        start = end + 1;
    }
    Coverage { allowed, uncertain }
}
