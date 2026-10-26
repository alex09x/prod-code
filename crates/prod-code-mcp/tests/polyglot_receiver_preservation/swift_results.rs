/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::support::{available, compiled, convert};

#[path = "swift_alias_scope.rs"]
mod alias_scope;
#[path = "swift_statement_proof.rs"]
mod statement_proof;

const STATEMENTS: &str = "print(({ return c.value })())\n        print(c.value)";

async fn statements(prefix: &str, members: &str, result: &str) {
    let before = format!(
        "{prefix}class C {{\n    {members}\n    var value = 7\n    static func same(c: C) -> {result} {{\n        {STATEMENTS}\n    }}\n}}\nC.same(c: C())\n"
    );
    check(&before, false, "7\n7\n").await;
}

async fn check(before: &str, value: bool, expected: &str) {
    eprintln!("BEFORE:\n{before}");
    if available("swiftc") {
        assert_eq!(compiled("swiftc", "swift", before), expected);
    } else {
        eprintln!("Swift compiler unavailable; no Swift runtime proof");
    }
    let (after, _) = convert(before, "swift", true).await;
    eprintln!("AFTER:\n{after}");
    assert!(
        after.contains("let c: C") && after.contains(" = self;"),
        "{after}"
    );
    if value {
        assert!(after.contains("return c.value"), "{after}");
    } else {
        assert!(after.contains(STATEMENTS), "{after}");
        assert!(!after.contains("return print("), "{after}");
    }
    if available("swiftc") {
        assert_eq!(compiled("swiftc", "swift", &after), expected);
    }
}

#[tokio::test]
async fn review9_swift_unrelated_void_is_not_visible() {
    statements("enum Other { typealias Void = Int }\n", "", "Void").await;
}

#[tokio::test]
async fn review9_swift_shadowed_module_qualifier_returns_value() {
    let before = "class C {\n    enum Swift { typealias Void = Int }\n    var value = 7\n    static func same(c: C) -> Swift.Void { c.value }\n}\nprint(C.same(c: C()))\n";
    check(before, true, "7\n").await;
}

#[tokio::test]
async fn review9_swift_grouped_void_remains_unit() {
    statements("", "", "(Void)").await;
}

#[tokio::test]
async fn review9_swift_scope_and_grouping_controls() {
    for (prefix, members) in [
        ("", "enum Other { typealias Void = Int }"),
        ("struct Other<Void> {}\n", ""),
        ("func other<Void>(_ value: Void) {}\n", ""),
        ("func other() { typealias Void = Int }\n", ""),
        ("enum Other { enum Swift { typealias Void = Int } }\n", ""),
        (
            "struct Other {}\nextension Other { typealias Void = Int }\n",
            "",
        ),
        (
            "enum Other { class C {} }\nextension Other.C { typealias Void = Int }\n",
            "",
        ),
        ("// typealias Void = Int\n", "let text = \"enum Swift { }\""),
    ] {
        for result in ["Void", "((Void))", "(())", "((Swift.Void))"] {
            statements(prefix, members, result).await;
        }
    }
    statements("typealias Void = Int\n", "", "Swift.Void").await;
    statements("", "typealias Void = Int", "((Swift.Void))").await;
}

#[tokio::test]
async fn review9_swift_visible_bindings_keep_value_returns() {
    for (prefix, members, result) in [
        ("typealias Void = Int\n", "", "Void"),
        ("", "typealias Void = Int", "((Void))"),
        ("enum Swift { typealias Void = Int }\n", "", "Swift.Void"),
        ("", "enum Swift { typealias Void = Int }", "((Swift.Void))"),
    ] {
        let before = format!(
            "{prefix}class C {{\n    {members}\n    var value = 7\n    static func same(c: C) -> {result} {{ c.value }}\n}}\nprint(C.same(c: C()))\n"
        );
        check(&before, true, "7\n").await;
    }
    for (prefix, parameter, result) in [
        ("", "Void", "Void"),
        (
            "protocol Result { associatedtype Void }\n",
            "Swift: Result",
            "Swift.Void",
        ),
    ] {
        let argument = parameter.split(':').next().unwrap();
        let before = format!(
            "{prefix}class C <{parameter}> {{\n    var value: {result}\n    init(_ value: {result}) {{ self.value = value }}\n    static func same(c: C<{argument}>) -> {result} {{ c.value }}\n}}\n"
        );
        check(&before, true, "").await;
    }
    let before = "class Outer {\n    typealias Void = Int\n    class C {\n        var value = 7\n        static func same(c: C) -> Void { c.value }\n    }\n    static func run() { print(C.same(c: C())) }\n}\nOuter.run()\n";
    check(before, true, "7\n").await;
    let (after, _) = convert(before, "swift", false).await;
    assert!(after.contains("print(C().same())"), "{after}");
    assert!(after.ends_with("Outer.run()\n"), "{after}");
}

#[tokio::test]
async fn review10_swift_extension_bindings_keep_value_returns() {
    for (members, result) in [
        ("typealias Void = Int", "Void"),
        ("enum Swift { typealias Void = Int }", "Swift.Void"),
    ] {
        let before = format!(
            "class C {{\n    var value = 7\n    static func same(c: C) -> {result} {{ c.value }}\n}}\nextension C {{\n    {members}\n}}\nprint(C.same(c: C()))\n"
        );
        check(&before, true, "7\n").await;
    }
    for owner in ["Outer", "Outer.C"] {
        let before = format!(
            "class Outer {{\n    class C {{\n        var value = 7\n        static func same(c: C) -> Void {{ c.value }}\n    }}\n    static func run() {{ print(C.same(c: C())) }}\n}}\nextension {owner} {{\n    typealias Void = Int\n}}\nOuter.run()\n"
        );
        check(&before, true, "7\n").await;
        let (after, _) = convert(&before, "swift", false).await;
        assert!(after.contains("print(C().same())"), "{after}");
    }
}

#[tokio::test]
async fn review11_swift_primary_members_visible_from_extension() {
    for (members, result) in [
        ("typealias Void = Int", "Void"),
        ("enum Swift { typealias Void = Int }", "Swift.Void"),
    ] {
        let before = format!(
            "class Outer {{ {members} }}\nextension Outer {{\n    class C {{\n        var value = 7\n        static func same(c: C) -> {result} {{ c.value }}\n    }}\n    static func run() {{ print(C.same(c: C())) }}\n}}\nOuter.run()\n"
        );
        check(&before, true, "7\n").await;
    }
}

#[tokio::test]
async fn review11_swift_inherited_bindings_keep_value_returns() {
    for (prefix, base, result) in [
        ("class Base { typealias Void = Int }\n", "Base", "Void"),
        (
            "class Base { enum Swift { typealias Void = Int } }\n",
            "Base",
            "Swift.Void",
        ),
        (
            "class Root {}\nextension Root { typealias Void = Int }\nclass Base: Root {}\n",
            "Base",
            "Void",
        ),
        (
            "enum Other { class Base { typealias Void = Int } }\n",
            "Other.Base",
            "Void",
        ),
        (
            "class Base<T> { typealias Void = T }\n",
            "Base<Int>",
            "Void",
        ),
        (
            "class Base { typealias Void = Int }\ntypealias Parent = Base\n",
            "Parent",
            "Void",
        ),
    ] {
        let before = format!(
            "{prefix}class C: {base} {{\n    var value = 7\n    static func same(c: C) -> {result} {{ c.value }}\n}}\nprint(C.same(c: C()))\n"
        );
        check(&before, true, "7\n").await;
    }
}

#[tokio::test]
async fn review11_swift_inherited_unit_controls() {
    for (prefix, base) in [
        ("class Base {}\n", "Base"),
        (
            "class Base {}\nenum Other { class Base { typealias Void = Int } }\n",
            "Base",
        ),
        ("enum Other { class Base {} }\n", "Other.Base"),
    ] {
        let before = format!(
            "{prefix}class C: {base} {{\n    var value = 7\n    static func same(c: C) -> Void {{\n        {STATEMENTS}\n    }}\n}}\nC.same(c: C())\n"
        );
        check(&before, false, "7\n7\n").await;
    }
}

#[tokio::test]
async fn review9_swift_nonunit_grouped_types_keep_value_returns() {
    for (result, value) in [
        ("(Void)?", "nil"),
        ("(Void?)", "nil"),
        ("(Void, Void)", "((), ())"),
        ("(Void) -> Void", "{ _ in }"),
        ("(() -> Void)", "{}"),
    ] {
        let before = format!(
            "class C {{\n    var value: {result} = {value}\n    static func same(c: C) -> {result} {{ c.value }}\n}}\nC.same(c: C())\n"
        );
        check(&before, true, "").await;
    }
}

#[tokio::test]
async fn review12_swift_external_conformance_unit_statements() {
    for result in ["Swift.Void", "Void", "((Swift.Void))"] {
        let before = format!(
            "class C: CustomStringConvertible {{\n    var description: String {{ \"C\" }}\n    var value = 7\n    static func same(c: C) -> {result} {{\n        {STATEMENTS}\n    }}\n}}\nC.same(c: C())\n"
        );
        check(&before, false, "7\n7\n").await;
    }
    let before = format!(
        "class C: Equatable {{\n    static func == (lhs: C, rhs: C) -> Bool {{ lhs.value == rhs.value }}\n    var value = 7\n    static func same(c: C) -> Swift.Void {{\n        {STATEMENTS}\n    }}\n}}\nC.same(c: C())\n"
    );
    check(&before, false, "7\n7\n").await;
}

#[tokio::test]
async fn review12_swift_alias_extension_value_returns() {
    for (aliases, owner, members, result) in [
        (
            "typealias Alias = C",
            "Alias",
            "typealias Void = Int",
            "Void",
        ),
        (
            "typealias Alias = (C)",
            "Alias",
            "typealias Void = Int",
            "Void",
        ),
        (
            "typealias Alias = C\ntypealias Chain = Alias",
            "Chain",
            "enum Swift { typealias Void = Int }",
            "Swift.Void",
        ),
        (
            "enum Names { typealias Alias = C }",
            "Names.Alias",
            "typealias Void = Int",
            "((Void))",
        ),
    ] {
        let before = format!(
            "class C {{\n    var value = 7\n    static func same(c: C) -> {result} {{ c.value }}\n}}\n{aliases}\nextension {owner} {{ {members} }}\nprint(C.same(c: C()))\n"
        );
        check(&before, true, "7\n").await;
    }
    let before = "class Other {}\ntypealias Alias = Other\nclass Outer {\n    typealias Alias = C\n    class C {\n        var value = 7\n        static func same(c: C) -> Void { c.value }\n    }\n    static func run() { print(C.same(c: C())) }\n}\nextension Outer.Alias { typealias Void = Int }\nOuter.run()\n";
    check(before, true, "7\n").await;
}

#[tokio::test]
async fn review12_swift_unrelated_alias_extensions_remain_excluded() {
    for prefix in [
        "class Other {}\ntypealias Alias = Other\nextension Alias { typealias Void = Int }\n",
        "class Other {}\ntypealias Alias = Other\nenum Names { typealias Alias = C }\nextension Alias { enum Swift { typealias Void = Int } }\n",
        "import Foundation\ntypealias Alias = NSObject\nextension Alias { typealias Void = Int }\n",
    ] {
        for result in ["Void", "Swift.Void"] {
            statements(prefix, "", result).await;
        }
    }
}

#[tokio::test]
async fn review12_swift_external_conformance_single_expression_stays_safe() {
    for expression in ["c.value", "({ return c.value })()"] {
        let before = format!(
            "class C: CustomStringConvertible {{\n    var description: String {{ \"C\" }}\n    var value: () = ()\n    static func same(c: C) -> Swift.Void {{ {expression} }}\n}}\nC.same(c: C())\n"
        );
        if available("swiftc") {
            assert_eq!(compiled("swiftc", "swift", &before), "");
        }
        let (after, _) = convert(&before, "swift", true).await;
        assert!(after.contains(&format!("return {expression}")), "{after}");
        if available("swiftc") {
            assert_eq!(compiled("swiftc", "swift", &after), "");
        }
    }
    for (members, result) in [
        ("typealias Void = Int", "Void"),
        ("enum Swift { typealias Void = Int }", "Swift.Void"),
    ] {
        let before = format!(
            "class C: CustomStringConvertible {{\n    var description: String {{ \"C\" }}\n    {members}\n    var value = 7\n    static func same(c: C) -> {result} {{ c.value }}\n}}\nprint(C.same(c: C()))\n"
        );
        check(&before, true, "7\n").await;
    }
    let before = "class C: CustomStringConvertible {\n    var description: String { \"C\" }\n    var value = 7\n    static func same(c: C) -> Swift.Void {\n        print(\n            ({ return c.value })()\n        )\n    }\n}\n";
    super::support::refuses(before, "swift").await;
}

#[tokio::test]
async fn review13_swift_qualified_prefix_aliases_keep_value_returns() {
    for aliases in [
        "enum Names { typealias Alias = C }\ntypealias NamesAlias = Names",
        "typealias Target = C\nenum Names { typealias Alias = Target }\ntypealias NamesAlias = Names",
        "enum Names { typealias Alias = C }\ntypealias Prefix = Names\ntypealias NamesAlias = Prefix",
    ] {
        for (members, result) in [
            ("typealias Void = Int", "Void"),
            ("enum Swift { typealias Void = Int }", "Swift.Void"),
        ] {
            let before = format!(
                "class C {{\n    var value = 7\n    static func same(c: C) -> {result} {{ c.value }}\n}}\n{aliases}\nextension NamesAlias.Alias {{ {members} }}\nprint(C.same(c: C()))\n"
            );
            check(&before, true, "7\n").await;
        }
    }
}

#[tokio::test]
async fn review13_swift_unrelated_external_alias_keeps_multiline_unit_call() {
    for prefix in [
        "typealias Alias = Int\nextension Alias {}\n",
        "typealias Alias = Int\nextension Alias { enum Other { typealias Void = Int } }\n",
        "enum Names { typealias Alias = Int }\ntypealias NamesAlias = Names\nextension NamesAlias.Alias {}\n",
    ] {
        for result in ["Void", "((Swift.Void))"] {
            let before = format!(
                "{prefix}class C {{\n    var value = 7\n    static func same(c: C) -> {result} {{\n        print(\n            c.value\n        )\n    }}\n}}\nC.same(c: C())\n"
            );
            eprintln!("BEFORE:\n{before}");
            if available("swiftc") {
                assert_eq!(compiled("swiftc", "swift", &before), "7\n");
            }
            let (after, _) = convert(&before, "swift", true).await;
            eprintln!("AFTER:\n{after}");
            assert!(
                after.contains("let c: C") && after.contains(" = self;"),
                "{after}"
            );
            assert!(
                after.contains("print(\n            c.value\n        )"),
                "{after}"
            );
            assert!(!after.contains("return print("), "{after}");
            if available("swiftc") {
                assert_eq!(compiled("swiftc", "swift", &after), "7\n");
            }
        }
    }
}

#[tokio::test]
async fn review13_swift_incomplete_alias_expansion_stays_uncertain() {
    // These incomplete/invalid declarations exercise conservative conversion,
    // rather than claim Swift compilation of cyclic or unsupported alias types.
    for target in ["NamesAlias.Alias", "Names<Int>", "External"] {
        for (members, result) in [
            ("typealias Void = Int", "Void"),
            ("enum Swift { typealias Void = Int }", "Swift.Void"),
        ] {
            let before = format!(
                "class C {{\n    var value = 7\n    static func same(c: C) -> {result} {{\n        print(\n            c.value\n        )\n    }}\n}}\nenum Names {{ typealias Alias = {target} }}\ntypealias NamesAlias = Names\nextension NamesAlias.Alias {{ {members} }}\n"
            );
            super::support::refuses(&before, "swift").await;
        }
    }
}
