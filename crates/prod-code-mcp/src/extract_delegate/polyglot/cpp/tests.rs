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
fn declaration_selection_refuses_malformed_and_wrong_locations() {
    for (source, symbol, line, expected) in [
        (
            "",
            None,
            None,
            "no class/struct found at the specified location",
        ),
        (
            "class Store {}",
            Some("Other"),
            None,
            "class or struct `Other` not found",
        ),
        (
            "class Store {}",
            Some("Store"),
            Some(9),
            "class or struct `Store` not found",
        ),
        (
            "class Store",
            Some("Store"),
            None,
            "class or struct `Store` not found",
        ),
        (
            "class Store {",
            Some("Store"),
            None,
            "class or struct `Store` not found",
        ),
        (
            "subclass Store {}; class  {};",
            None,
            None,
            "no class/struct found at the specified location",
        ),
        (
            "struct Store {};",
            Some("Store"),
            None,
            "`Store` has no field `value`",
        ),
        (
            "struct Store {\n int value;\n Details details;\n};",
            Some("Store"),
            None,
            "`Store` already has a field `details`",
        ),
    ] {
        assert_eq!(
            restructure_cpp(
                source,
                symbol,
                line,
                &["value".into()],
                &[],
                "Details",
                "details"
            )
            .unwrap_err()
            .to_string(),
            expected
        );
    }
}
