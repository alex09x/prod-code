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
fn cpp_refusals_have_specific_messages() {
    for (text, expected) in [
        ("", "class/struct `Store` not found in C++ source"),
        ("class Store", "class body opening brace not found"),
        (
            "class Store {",
            "class body matching closing brace not found",
        ),
        (
            "struct Store {\n Store();\n ~Store();\n private:\n protected:\n // comment\n int field;\n};",
            "no matching member functions found in `Store` to extract",
        ),
        ("class Store {\n int read(\n};", "malformed parameter list"),
    ] {
        assert_eq!(
            extract_interface_cpp(text, "Store", "Reader", &[])
                .unwrap_err()
                .to_string(),
            expected
        );
    }
}

#[test]
fn empty_inheritance_and_const_signature_render_exactly() {
    let (out, methods) = extract_interface_cpp(
        "struct Store : {\n inline int read() const { return 1; }\n int clear() {}\n};",
        "Store",
        "Reader",
        &["read".into()],
    )
    .unwrap();
    assert_eq!(methods, ["read"]);
    assert_eq!(
        out,
        "class Reader {\npublic:\n    virtual ~Reader() = default;\n    virtual int read() const = 0;\n};\n\nstruct Store: public Reader {\n inline int read() const { return 1; }\n int clear() {}\n};"
    );
    for (header, expected) in [
        ("class Store<ns::T>", None),
        ("class Store<ns::T>: public Base", Some(18)),
        ("class ns::Store", None),
    ] {
        assert_eq!(cpp_inheritance_separator(header), expected, "{header}");
    }
}
