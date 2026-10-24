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
fn swift_headers_keep_generic_constraints_and_base_order() {
    for (header, expected) in [
        ("struct Store", "struct Store: Reader"),
        (
            "struct Store: Codable, Sendable",
            "struct Store: Codable, Sendable, Reader",
        ),
        (
            "class Store: Base<A, B>, Codable",
            "class Store: Base<A, B>, Reader, Codable",
        ),
        (
            "struct Store<T: P> where T: Q",
            "struct Store<T: P>: Reader where T: Q",
        ),
        (
            "class Store<T>: Base<T> where T: Q",
            "class Store<T>: Base<T>, Reader where T: Q",
        ),
        ("struct Store:", "struct Store: Reader"),
    ] {
        let text = format!(
            "// café\r\n{header} {{\n    func read() -> Int {{ return 1 }}\n    mutating func clear() {{}}\n}}\n"
        );
        let (out, methods) = extract_interface_swift(&text, "Store", "Reader", &[]).unwrap();
        assert_eq!(methods, ["read", "clear"]);
        assert_eq!(
            out,
            format!(
                "// café\r\nprotocol Reader {{\n    func read() -> Int\n    mutating func clear()\n}}\n\n{expected} {{\n    func read() -> Int {{ return 1 }}\n    mutating func clear() {{}}\n}}\n"
            )
        );
    }
}

#[test]
fn swift_selection_and_malformed_declarations() {
    let text = "struct Store {\n    let value = 1\n    func broken\n    func read() {}\n    func clear() {}\n}";
    let (out, names) = extract_interface_swift(text, "Store", "Reader", &["read".into()]).unwrap();
    assert_eq!(names, ["read"]);
    assert_eq!(
        out,
        "protocol Reader {\n    func read()\n}\n\nstruct Store: Reader {\n    let value = 1\n    func broken\n    func read() {}\n    func clear() {}\n}"
    );
    for (text, methods, expected) in [
        ("", vec![], "class/struct `Store` not found in Swift source"),
        ("class Store", vec![], "type body opening brace not found"),
        (
            "class Store {",
            vec![],
            "type body matching closing brace not found",
        ),
        (
            "class Store {}",
            vec![],
            "no matching methods found in `Store` to extract",
        ),
        (
            text,
            vec!["missing".into()],
            "no matching methods found in `Store` to extract",
        ),
    ] {
        assert_eq!(
            extract_interface_swift(text, "Store", "Reader", &methods)
                .unwrap_err()
                .to_string(),
            expected
        );
    }
}

#[test]
fn swift_header_scanners_respect_nesting_and_word_boundaries() {
    for (header, separator, where_start) in [
        ("", None, None),
        ("Store<T: P>", None, None),
        ("Store<T: P>: Base<T>", Some(11), None),
        ("where T: P", Some(7), Some(0)),
        ("Somewhere T", None, None),
        ("Store<T where U>: P where T: Q", Some(16), Some(20)),
        (">:", Some(1), None),
    ] {
        assert_eq!(swift_inheritance_separator(header), separator, "{header}");
        assert_eq!(swift_where_clause_start(header), where_start, "{header}");
    }
    for (input, expected) in [
        ("", vec![]),
        (
            " , Base<A, B>, , Café, Pair<X<Y>, Z>,",
            vec!["Base<A, B>", "Café", "Pair<X<Y>, Z>"],
        ),
        (">, P", vec![">", "P"]),
    ] {
        assert_eq!(split_swift_inheritance(input), expected);
    }
}
