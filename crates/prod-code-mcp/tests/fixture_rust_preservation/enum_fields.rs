/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{compiles, fixture};

#[tokio::test]
async fn raw_record_field_does_not_hide_unit_variant() {
    let source = "enum Event { Empty, Data { r#type: u8 } }\n";
    let (ws, result) = fixture(source, "Event", 2).await;
    compiles(
        &ws,
        source,
        &result,
        "assert!(matches!(value, Event::Empty));",
    );
    assert_eq!(result.value, "Event::Empty");
    assert!(result.fallbacks.is_empty());
}

#[tokio::test]
async fn payload_only_raw_record_fields_retain_spelling() {
    let source = "enum Event { Data { r#type: u8, r#match: String } }\n";
    let (ws, result) = fixture(source, "Event", 2).await;
    compiles(
        &ws,
        source,
        &result,
        "assert!(matches!(value, Event::Data { r#type: 0, r#match: s } if s.is_empty()));",
    );
    assert!(result.value.contains("r#type: 0,"));
    assert!(result.value.contains("r#match: String::new(),"));
    assert!(result.fallbacks.is_empty());
}

#[tokio::test]
async fn function_return_arrows_do_not_split_enum_payloads() {
    for source in [
        "enum Event { Empty, Data { callback: fn() -> u8, flag: bool } }\n",
        "enum Event { Empty, Data(fn() -> u8, bool) }\n",
        "enum Event { Empty, Data { callback: fn() -> Result<u8, ()>, flag: bool } }\n",
    ] {
        let (ws, result) = fixture(source, "Event", 2).await;
        compiles(
            &ws,
            source,
            &result,
            "assert!(matches!(value, Event::Empty));",
        );
        assert_eq!(result.value, "Event::Empty");
        assert!(result.fallbacks.is_empty());
    }
}
