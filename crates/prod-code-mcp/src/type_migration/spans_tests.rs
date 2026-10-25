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
fn coverage_w2_declared_type_spans_handle_modifiers_and_refuse_untyped_headers() {
    use Language::*;
    for (lang, text, name, expected) in [
        (
            Cpp,
            "virtual static inline constexpr friend long target();",
            "target",
            Some("long"),
        ),
        (
            C,
            "extern static unsigned long target;",
            "target",
            Some("unsigned long"),
        ),
        (Java, "String target;", "target", Some("String")),
        (Cpp, "static target();", "target", None),
        (Cpp, "target();", "target", None),
        (Cpp, "auto target = value;", "target", None),
        (C, "target = value;", "target", None),
        (Go, "func target() {}", "target", None),
        (Go, "target := value", "target", None),
        (Rust, "fn target() {} fn next() -> u8 {}", "target", None),
        (Rust, "fn target(", "target", None),
        (Python, "def target(): pass", "target", None),
        (TypeScript, "function target()", "target", None),
        (TypeScript, "function target() {}", "target", None),
        (JavaScript, "const target = 3;", "target", None),
        (
            TypeScript,
            "target ? : Map<string, number>;",
            "target",
            Some("Map<string, number>"),
        ),
        (
            TypeScript,
            "target(): () => number;",
            "target",
            Some("() => number"),
        ),
        (TypeScript, "target(): number;", "target", Some("number")),
        (
            Python,
            "def target() -> list[int]  : pass",
            "target",
            Some("list[int]"),
        ),
    ] {
        let span = declared_type_span_polyglot(text, text.find(name).unwrap(), lang);
        assert_eq!(span.map(|(s, e)| &text[s..e]), expected, "{lang:?}: {text}");
    }
    assert_eq!(end_of_type("Map<A, B>", 0, b","), None);
    assert_eq!(end_of_type("A\nnext", 0, b","), Some(1));
    assert_eq!(end_of_type("(x: A) => B;", 0, b";="), Some(11));
}

#[test]
fn coverage_w2_decl_lookup_ignores_imports_comments_and_identifier_fragments() {
    for (lang, import) in [
        (Language::Rust, "pub(crate) use value;"),
        (Language::Python, "from other import value"),
        (Language::TypeScript, "import{value} from './other';"),
        (Language::Go, "import (value)"),
        (Language::Cpp, "using value = Other;"),
        (Language::Swift, "import value"),
        (Language::Java, "package value;"),
    ] {
        assert!(is_import_line(import, import.find("value").unwrap(), lang));
        assert!(!is_import_line("value + 1", 0, lang));
    }
    let text = "use value;\n// value: Wrong\nlet prefix_value: Wrong = 1;\nlet value_suffix: Wrong = 1;\nlet value: u8 = 1;\nlet value: u16 = 2;\n";
    let first = text.find("value: u8").unwrap();
    assert_eq!(
        find_symbol_decl_offset(text, "value", Language::Rust, None),
        Some(first)
    );
    assert_eq!(
        find_symbol_decl_offset(text, "value", Language::Rust, Some(0)),
        Some(first)
    );
    assert_eq!(
        find_symbol_decl_offset(text, "value", Language::Rust, Some(999)),
        Some(first)
    );
    assert_eq!(
        find_symbol_decl_offset(text, "value", Language::Rust, Some(6)),
        Some(first)
    );
    let near_import = "// value: Wrong\nuse value;\nlet value: u8 = 1;";
    assert_eq!(
        find_symbol_decl_offset(near_import, "value", Language::Rust, Some(1)),
        near_import.find("value: u8")
    );
    assert_eq!(
        find_symbol_decl_offset("// value\n", "value", Language::Rust, None),
        None
    );
}
