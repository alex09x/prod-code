/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::fixture::builder::lexer::Source;

/// Byte offset after file comments and inner attributes, leaving outer item docs/attributes
/// attached to their declaration. Inner attributes may occupy several lines (#1147).
pub(super) fn preamble_end(text: &str) -> usize {
    let mut at = 0;
    loop {
        at += text[at..].len() - text[at..].trim_start().len();
        let rest = &text[at..];
        if rest.starts_with("//") && !rest.starts_with("///") {
            at += rest.find('\n').map_or(rest.len(), |i| i + 1);
        } else if rest.starts_with("/*") && !rest.starts_with("/**") {
            let mut depth = 1;
            let mut end = 2;
            while end < rest.len() && depth > 0 {
                if rest[end..].starts_with("/*") {
                    depth += 1;
                    end += 2;
                } else if rest[end..].starts_with("*/") {
                    depth -= 1;
                    end += 2;
                } else {
                    end += rest[end..].chars().next().unwrap().len_utf8();
                }
            }
            at += end;
        } else if rest.starts_with("#![") {
            let Ok(source) = Source::new(rest) else { break };
            let Some(close) = source.close_of(2) else {
                break;
            };
            at += source.tokens[close].end;
        } else {
            return at;
        }
    }
    at
}
