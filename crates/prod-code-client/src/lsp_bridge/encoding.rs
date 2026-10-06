/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub const POSITION_ENCODING_UTF16: u8 = 0;
pub const POSITION_ENCODING_UTF8: u8 = 1;
pub const POSITION_ENCODING_UTF32: u8 = 2;

pub fn lsp_position_encoding(message: &serde_json::Value) -> Option<u8> {
    match message
        .pointer("/result/capabilities/positionEncoding")
        .and_then(serde_json::Value::as_str)
    {
        Some("utf-8") => Some(POSITION_ENCODING_UTF8),
        Some("utf-16") => Some(POSITION_ENCODING_UTF16),
        Some("utf-32") => Some(POSITION_ENCODING_UTF32),
        _ => None,
    }
}

pub fn lsp_offset(
    text: &str,
    target_line: usize,
    target_col: usize,
    position_encoding: u8,
) -> usize {
    let mut current_line = 0;
    let mut current_col = 0;
    for (offset, ch) in text.char_indices() {
        if current_line == target_line && current_col == target_col {
            return offset;
        }
        if ch == '\n' {
            current_line += 1;
            current_col = 0;
        } else {
            current_col += match position_encoding {
                POSITION_ENCODING_UTF8 => ch.len_utf8(),
                POSITION_ENCODING_UTF32 => 1,
                _ => ch.len_utf16(),
            };
        }
    }
    text.len()
}
