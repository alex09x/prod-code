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

/// Computes a boolean mask indicating code bytes (true) versus comments/strings (false).
pub fn lexical_code_mask(text: &str, lang: Language) -> Vec<bool> {
    let bytes = text.as_bytes();
    let mut code = vec![true; bytes.len()];
    let mut i = 0usize;

    while i < bytes.len() {
        if lang == Language::Python && bytes[i] == b'#' {
            let end = text[i..].find('\n').map_or(bytes.len(), |n| i + n);
            code[i..end].fill(false);
            i = end;
            continue;
        }

        if lang != Language::Python && bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'/') {
            let end = text[i..].find('\n').map_or(bytes.len(), |n| i + n);
            code[i..end].fill(false);
            i = end;
            continue;
        }

        if lang != Language::Python && bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
            let mut j = i + 2;
            let mut depth = 1usize;
            while j < bytes.len() && depth > 0 {
                if bytes[j] == b'/' && bytes.get(j + 1) == Some(&b'*') {
                    depth += 1;
                    j += 2;
                } else if bytes[j] == b'*' && bytes.get(j + 1) == Some(&b'/') {
                    depth -= 1;
                    j += 2;
                } else {
                    j += 1;
                }
            }
            code[i..j].fill(false);
            i = j;
            continue;
        }

        // Strings
        let is_quote = if lang == Language::Python {
            bytes[i] == b'"' || bytes[i] == b'\''
        } else if lang == Language::TypeScript || lang == Language::JavaScript {
            bytes[i] == b'"' || bytes[i] == b'\'' || bytes[i] == b'`'
        } else {
            bytes[i] == b'"'
        };

        if is_quote {
            let quote = bytes[i];
            let mut j = i + 1;
            while j < bytes.len() {
                if bytes[j] == b'\\' {
                    j = (j + 2).min(bytes.len());
                } else if bytes[j] == quote {
                    j += 1;
                    break;
                } else {
                    j += 1;
                }
            }
            code[i..j].fill(false);
            i = j;
            continue;
        }

        i += 1;
    }

    code
}
