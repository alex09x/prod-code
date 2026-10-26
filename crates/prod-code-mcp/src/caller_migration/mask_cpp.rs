/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

/// C++ character literals use the ordinary escape-aware quote scanner. An
/// apostrophe inside a numeric preprocessing token is instead a digit separator.
pub(super) fn character_quote(bytes: &[u8], at: usize) -> bool {
    if bytes[at] != b'\'' {
        return false;
    }
    let mut start = at;
    while start > 0 {
        let previous = bytes[start - 1];
        // A preprocessing number includes its fractional portion and an optional
        // sign immediately following an exponent marker, including hexadecimal p/P.
        if previous.is_ascii_alphanumeric() || matches!(previous, b'_' | b'\'' | b'.') {
            start -= 1;
        } else if matches!(previous, b'+' | b'-')
            && start >= 2
            && matches!(bytes[start - 2], b'e' | b'E' | b'p' | b'P')
        {
            start -= 2;
        } else {
            break;
        }
    }
    !(bytes[start].is_ascii_digit()
        || (bytes[start] == b'.' && bytes.get(start + 1).is_some_and(u8::is_ascii_digit)))
}
