/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use serde_json::json;

use crate::refactor::locations::lsp_locations;

#[test]
fn malformed_required_locations_return_errors_without_panicking() {
    for n in [u32::MAX as u64, u32::MAX as u64 + 1, u64::MAX] {
        for key in ["line", "character"] {
            let mut location =
                json!({"uri": "file:///tmp/a.rs", "range": {"start": {"line": 0, "character": 0}}});
            location["range"]["start"][key] = json!(n);
            let result =
                std::panic::catch_unwind(|| lsp_locations(&json!([location]), "implementations"));
            assert!(result.is_ok(), "coordinate {key}={n} panicked");
            assert!(
                result.unwrap().is_err(),
                "coordinate {key}={n} was accepted"
            );
        }
    }
    for uri in [
        "https://example.invalid/a.rs",
        "file:///tmp/a.rs?version=2",
        "file:///tmp/a.rs#part",
        "file://remote.invalid/a.rs",
        "relative.rs",
    ] {
        let location = json!({"uri": uri, "range": {"start": {"line": 0, "character": 0}}});
        assert!(
            lsp_locations(&json!([location]), "definitions").is_err(),
            "accepted {uri}"
        );
    }
}

#[test]
fn required_locations_accept_protocol_empty_answers_and_encoded_local_files() {
    assert!(
        lsp_locations(&serde_json::Value::Null, "declarations")
            .unwrap()
            .is_empty()
    );
    assert!(
        lsp_locations(&json!([]), "declarations")
            .unwrap()
            .is_empty()
    );
    let path = std::env::temp_dir().join("a # %41 ü.rs");
    let uri = url::Url::from_file_path(&path).unwrap().to_string();
    for answer in [
        json!({"uri": uri, "range": {"start": {"line": 2, "character": 3}}}),
        json!({"targetUri": uri, "targetSelectionRange": {"start": {"line": 2, "character": 3}}}),
    ] {
        assert_eq!(
            lsp_locations(&answer, "declarations").unwrap(),
            vec![(path.clone(), 3, 4)]
        );
    }
}
