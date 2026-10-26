/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub(super) fn has_raw_string(text: &str) -> bool {
    scan(text).1
}

/// Receiver conversion only needs code boundaries, not a C++ expression parser.
/// Raw literals remain unsupported: callers must refuse before using their mask.
pub(super) fn scan(text: &str) -> (Vec<bool>, bool) {
    let bytes = text.as_bytes();
    let mut code = vec![true; bytes.len()];
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        if bytes[i..].starts_with(b"//") {
            i = text[i..].find('\n').map_or(bytes.len(), |n| i + n);
        } else if bytes[i..].starts_with(b"/*") {
            // C++ comments do not nest, even when their text contains another /*.
            i = text[i + 2..]
                .find("*/")
                .map_or(bytes.len(), |n| i + 2 + n + 2);
        } else if bytes[i].is_ascii_digit() {
            // Apostrophes in preprocessing numbers are digit separators, not
            // character delimiters (including hexadecimal and suffixed values).
            i += 1;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || matches!(bytes[i], b'_' | b'\'' | b'.'))
            {
                i += 1;
            }
            continue;
        } else if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' || bytes[i] >= 128 {
            if ["R\"", "u8R\"", "uR\"", "UR\"", "LR\""]
                .iter()
                .any(|prefix| bytes[i..].starts_with(prefix.as_bytes()))
            {
                // This is a raw-literal prefix at a token boundary, outside
                // comments/strings. Do not guess at any subsequent delimiters.
                code[i..].fill(false);
                return (code, true);
            }
            i += 1;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] >= 128)
            {
                i += 1;
            }
            continue;
        } else if matches!(bytes[i], b'\'' | b'"') {
            let quote = bytes[i];
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' {
                    i = (i + 2).min(bytes.len());
                } else if bytes[i] == quote {
                    i += 1;
                    break;
                } else {
                    i += 1;
                }
            }
        } else {
            i += 1;
            continue;
        }
        code[start..i].fill(false);
    }
    (code, false)
}
