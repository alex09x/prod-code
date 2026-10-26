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
fn coverage_import_dispatch_and_receivers() {
    for (source, start, name) in [
        ("α.pkg", 0, "α.pkg"),
        ("x ->ptr", 4, "ptr"),
        ("ns::item", 0, "ns::item"),
        ("ptr->item", 0, "ptr->item"),
        ("x..item", 3, "item"),
        ("", 0, ""),
    ] {
        assert_eq!(find_dotted_receiver_span(source), (start, name));
    }
    for (text, expected) in [
        ("<header.hpp>", "header.hpp"),
        ("<unfinished", "unfinished"),
        ("'unfinished", "unfinished"),
        ("`module`", "module"),
        ("no quote", ""),
    ] {
        assert_eq!(extract_specifier(text), expected);
    }
    assert_eq!(
        imported_caller_symbols(
            "",
            Path::new("caller"),
            Path::new("/"),
            "f",
            Language::Swift
        ),
        Vec::<String>::new()
    );
    assert_eq!(
        imported_caller_symbols(
            "",
            Path::new("pkg/caller.swift"),
            Path::new("pkg/value.swift"),
            "f",
            Language::Swift
        ),
        Vec::<String>::new()
    );
    assert_eq!(
        imported_caller_symbols(
            "",
            Path::new("pkg/caller.swift"),
            Path::new("other/value.swift"),
            "f",
            Language::Swift
        ),
        Vec::<String>::new()
    );
    assert!(!is_proven_namespace_import(
        "",
        "",
        Path::new("a.ts"),
        Path::new("b.ts"),
        Language::TypeScript
    ));
    assert!(!is_proven_namespace_import(
        "",
        "api",
        Path::new("a.swift"),
        Path::new("b.swift"),
        Language::Swift
    ));
}
