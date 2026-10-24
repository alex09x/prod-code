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

#[test]
fn class_selection_and_field_refusals_are_precise() {
    for (text, symbol, line, fields, field, expected) in [
        (
            "",
            None,
            None,
            "value",
            "details",
            "no class found at the specified location",
        ),
        (
            "class Store {}",
            Some("Other"),
            None,
            "value",
            "details",
            "class `Other` not found",
        ),
        (
            "class Store {}",
            Some("Store"),
            Some(99),
            "value",
            "details",
            "class `Store` not found",
        ),
        (
            "class Store",
            Some("Store"),
            None,
            "value",
            "details",
            "class `Store` not found",
        ),
        (
            "class Store {",
            Some("Store"),
            None,
            "value",
            "details",
            "class `Store` not found",
        ),
        (
            "class  {}",
            None,
            None,
            "value",
            "details",
            "no class found at the specified location",
        ),
        (
            "subclass Store {}",
            None,
            None,
            "value",
            "details",
            "no class found at the specified location",
        ),
        (
            "class Store {}",
            Some("Store"),
            None,
            "value",
            "details",
            "`Store` has no field `value`",
        ),
        (
            "class Store {\n value: number;\n details: Details;\n}",
            Some("Store"),
            None,
            "value",
            "details",
            "`Store` already has a field `details`",
        ),
    ] {
        assert_eq!(
            restructure_ts(
                text,
                symbol,
                line,
                &[fields.into()],
                &[],
                "Details",
                field,
                false
            )
            .unwrap_err()
            .to_string(),
            expected
        );
    }
}

#[test]
fn javascript_fields_are_initialized_without_type_annotations() {
    let source = "class Store {\n    // field\n    value = 3;\n}";
    let (text, owner) = restructure_ts(
        source,
        Some("Store"),
        Some(2),
        &["value".into()],
        &[],
        "Details",
        "details",
        true,
    )
    .unwrap();
    assert_eq!(owner, "Store");
    assert_eq!(
        text,
        "class Details {\n    value;\n\n    constructor(value) {\n        this.value = value;\n    }\n}\n\nclass Store {\n\n    // field\n    details = new Details(3);\n}"
    );
}

#[test]
fn untyped_fields_and_constructor_assignments_use_fallback_types() {
    let source = "class Store {\n    constructor(value) {\n        this.value = value;\n    }\n}";
    let (text, owner) = restructure_ts(
        source,
        None,
        None,
        &["value".into()],
        &[],
        "Details",
        "details",
        false,
    )
    .unwrap();
    assert_eq!(owner, "Store");
    assert_eq!(
        text,
        "class Details {\n    public value: any;\n\n    constructor(value: any) {\n        this.value = value;\n    }\n}\n\nclass Store {\n    public details: Details;\n\n    constructor(value) {\n        this.details = new Details(value);\n    }\n}"
    );
}
