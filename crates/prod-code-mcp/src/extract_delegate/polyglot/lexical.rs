/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::parameter_object::{
    Language,
    syntax::{close_in, walk_code},
};

pub(super) fn trivia(mut text: &str, lang: Language) -> &str {
    loop {
        text = text.trim_start();
        if let Some(rest) = text.strip_prefix("/*") {
            let Some(end) = rest.find("*/") else {
                return text;
            };
            text = &rest[end + 2..];
        } else if text.starts_with("//") && lang != Language::Python
            || text.starts_with('#') && lang == Language::Python
        {
            let Some(end) = text.find('\n') else {
                return "";
            };
            text = &text[end + 1..];
        } else {
            return text;
        }
    }
}

/// Confirm that a returned position is code, including ordinary template/f-string
/// interpolation. The analyzer still supplies the identity; this only catches stale
/// positions that now land in comments or literal text.
pub(super) fn reference_is_code(text: &str, at: usize, lang: Language) -> bool {
    if matches!(lang, Language::TypeScript | Language::JavaScript) {
        return super::js_code::reference_is_code(text, at);
    }
    if lang == Language::Swift {
        return super::swift_code::reference_is_code(text, at);
    }
    let mut code = false;
    let mut quote_start = None;
    let mut containing_string = None;
    walk_code(text, 0, lang, |i, c| {
        if i == at {
            code = true;
            return false;
        }
        if matches!(c, b'\'' | b'"' | b'`') {
            if let Some(start) = quote_start.take() {
                if start < at && at < i {
                    containing_string = Some((start, i));
                    return false;
                }
            } else {
                quote_start = Some(i);
            }
        }
        i <= at || quote_start.is_some()
    });
    if code {
        return true;
    }
    let Some((open, close)) = containing_string else {
        return false;
    };
    let template = matches!(lang, Language::TypeScript | Language::JavaScript)
        && text.as_bytes()[open] == b'`';
    let f_string = lang == Language::Python
        && text[..open]
            .chars()
            .next_back()
            .is_some_and(|c| c == 'f' || c == 'F');
    if !template && !f_string {
        return false;
    }
    let marker = if template { "${" } else { "{" };
    let mut cursor = open + 1;
    while cursor < close {
        let Some(relative) = text[cursor..close].find(marker) else {
            break;
        };
        let marker_start = cursor + relative;
        let brace = marker_start + marker.len() - 1;
        if template
            && text[..marker_start]
                .bytes()
                .rev()
                .take_while(|b| *b == b'\\')
                .count()
                % 2
                == 1
        {
            cursor = brace + 1;
            continue;
        }
        let Some(end) = close_in(text, brace, lang) else {
            break;
        };
        if brace < at && at < end {
            return reference_is_code(&text[brace + 1..end], at - brace - 1, lang);
        }
        cursor = end + 1;
    }
    false
}
