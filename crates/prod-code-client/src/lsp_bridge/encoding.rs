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

/// Preserve the editor's original sync contract in the high bits while keeping
/// the negotiated character encoding in the low two bits.
pub fn update_server_capabilities(
    state: &std::sync::atomic::AtomicU8,
    message: &serde_json::Value,
    require_compatible: bool,
) -> bool {
    use std::sync::atomic::Ordering;
    let Some(capabilities) = message
        .pointer("/result/capabilities")
        .and_then(serde_json::Value::as_object)
    else {
        return false;
    };
    let encoding = lsp_position_encoding(message).unwrap_or(POSITION_ENCODING_UTF16);
    let sync = capabilities
        .get("textDocumentSync")
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.get("change").and_then(serde_json::Value::as_u64))
        })
        .filter(|value| *value <= 2)
        .unwrap_or(0) as u8;
    let previous = state.load(Ordering::Acquire);
    let original_sync = previous >> 2;
    if require_compatible
        && ((previous & 3) != encoding
            || (original_sync == 3 && sync != 2)
            || (original_sync == 2 && sync == 0))
    {
        return false;
    }
    let sync_field = if require_compatible && original_sync != 0 {
        original_sync
    } else {
        sync + 1
    };
    state.store(encoding | (sync_field << 2), Ordering::Release);
    true
}

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
            current_col += match position_encoding & 3 {
                POSITION_ENCODING_UTF8 => ch.len_utf8(),
                POSITION_ENCODING_UTF32 => 1,
                _ => ch.len_utf16(),
            };
        }
    }
    text.len()
}

#[cfg(test)]
#[path = "encoding_tests.rs"]
mod tests;
