/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;
use serde_json::json;

#[test]
fn negotiated_encodings_require_a_known_string_at_capabilities() {
    for (value, expected) in [
        (json!("utf-8"), Some(POSITION_ENCODING_UTF8)),
        (json!("utf-16"), Some(POSITION_ENCODING_UTF16)),
        (json!("utf-32"), Some(POSITION_ENCODING_UTF32)),
        (json!("UTF-8"), None),
        (json!(null), None),
        (json!(8), None),
    ] {
        assert_eq!(
            lsp_position_encoding(&json!({"result":{"capabilities":{"positionEncoding":value}}})),
            expected
        );
    }
    assert_eq!(
        lsp_position_encoding(&json!({"positionEncoding":"utf-8"})),
        None
    );
}

#[test]
fn offsets_follow_encoding_units_and_crlf_lines() {
    let text = "a😀界\r\nβz\n";
    for (encoding, column) in [
        (POSITION_ENCODING_UTF8, 5),
        (POSITION_ENCODING_UTF16, 3),
        (POSITION_ENCODING_UTF32, 2),
        (99, 3),
    ] {
        assert_eq!(lsp_offset(text, 0, column, encoding), 5);
        assert_eq!(lsp_offset(text, 1, 0, encoding), 10);
        assert_eq!(lsp_offset(text, 2, 0, encoding), text.len());
        assert_eq!(lsp_offset(text, 9, 0, encoding), text.len());
    }
    assert_eq!(lsp_offset(text, 1, 2, POSITION_ENCODING_UTF8), 12);
    assert_eq!(lsp_offset(text, 1, 1, POSITION_ENCODING_UTF32), 12);
    assert_eq!(lsp_offset("", 0, 0, POSITION_ENCODING_UTF16), 0);
    assert_eq!(
        lsp_offset(text, 0, 999, POSITION_ENCODING_UTF16),
        text.len()
    );
}

#[test]
fn capability_replay_preserves_editor_sync_contract() {
    use std::sync::atomic::{AtomicU8, Ordering};
    let state = AtomicU8::new(0);
    let response = |sync| serde_json::json!({"result":{"capabilities":{"positionEncoding":"utf-8","textDocumentSync":{"change":sync}}}});
    assert!(update_server_capabilities(&state, &response(1), false));
    assert!(update_server_capabilities(&state, &response(2), true));
    // The editor still sends full changes even if a later server supports incremental ones.
    assert!(update_server_capabilities(&state, &response(1), true));
    assert_eq!(lsp_offset("😀a", 0, 4, state.load(Ordering::Acquire)), 4);
    assert!(!update_server_capabilities(&state, &response(0), true));
    assert!(update_server_capabilities(&state, &response(2), false));
    assert!(!update_server_capabilities(&state, &response(1), true));
}
