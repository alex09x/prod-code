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
fn parse_parameters_ignores_receivers_and_separators() {
    type Parser = fn(&str) -> Vec<String>;
    let cases: &[(Parser, &str, &[&str])] = &[
        (
            parse_params_python,
            "self, /, , *, value: int=1, *args, **kwargs, bad-name",
            &["value", "args", "kwargs"],
        ),
        (
            parse_params_ts,
            "this: Store, public readonly value: number, private name = '', protected active: boolean, bad-name, ,",
            &["value", "name", "active"],
        ),
        (
            parse_params_go,
            " , value int, name string, bad-name bool",
            &["value", "name"],
        ),
        (
            parse_params_swift,
            " , _ value: Int, label name: String, self: Store, _: Int, bad-name: Int",
            &["value", "name"],
        ),
        (
            parse_params_cpp,
            " , const Thing &value, char *name = 0, this, bad-name",
            &["value", "name"],
        ),
    ];
    for (parse, input, expected) in cases {
        assert_eq!(parse(input), *expected, "{input}");
        assert_eq!(parse(""), Vec::<String>::new());
    }
}

#[test]
fn local_names_cover_each_declaration_form() {
    for (lang, source, expected) in [
        (
            Language::TypeScript,
            "const a: number = 1;\r\nlet b = 2;\nvar café = 3;\nconst [x] = xs;\nconst = 4",
            vec!["a", "b", "café"],
        ),
        (
            Language::JavaScript,
            "let a = 1;\nvar b = 2",
            vec!["a", "b"],
        ),
        (
            Language::Python,
            "# a = 0\nself.a = 1\nx == 2\na: int = 3\nb = 4\nx.y = 5\ndef = 6\nclass = 7",
            vec!["a", "b"],
        ),
        (
            Language::Go,
            "a := 1\nx, y := 2, 3\nbad-name := 4",
            vec!["a"],
        ),
        (
            Language::Swift,
            "let a: Int = 1\nvar b = 2\nlet (x,y) = pair",
            vec!["a", "b"],
        ),
        (
            Language::Cpp,
            "int *a = 0;\nThing &b = t;\nx = 2;\nint bad-name = 3;",
            vec!["a", "b"],
        ),
        (Language::C, "int a = 1;", vec!["a"]),
        (Language::Rust, "let x = 1;", vec![]),
        (Language::Java, "int x = 1;", vec![]),
    ] {
        assert_eq!(parse_locals(source, lang), expected, "{lang:?}");
    }
}

#[test]
fn member_detection_handles_declaration_forms_and_incomplete_types() {
    let cases = [
        (Language::Go, "type Store struct {\n value int\n}", true),
        (
            Language::Python,
            "class Store:\n # comment\n\n value: int\nclass Other:\n pass",
            true,
        ),
        (
            Language::Python,
            "class Store:\n pass\nclass Other:\n value = 1",
            false,
        ),
        (Language::Python, "class Store:\n self.value = 1", true),
        (
            Language::Python,
            "class Store:\n def value(self): pass",
            true,
        ),
        (
            Language::TypeScript,
            "class Store {\n private readonly value?: number;\n}",
            true,
        ),
        (
            Language::JavaScript,
            "class Store {\n static value = 1;\n}",
            true,
        ),
        (Language::Swift, "actor Store {\n func value() {}\n}", true),
        (Language::Swift, "struct Store {\n let value = 1\n}", true),
        (Language::Cpp, "struct Store {\n int value;\n};", true),
        (Language::C, "struct Store {\n int value = 0;\n};", true),
        (Language::Rust, "struct Store { value: i32 }", false),
        (Language::Java, "class Store { int value; }", false),
    ];
    for (lang, source, expected) in cases {
        assert_eq!(
            has_member_named(source, lang, "Store", "value"),
            expected,
            "{source}"
        );
        assert!(!has_member_named(source, lang, "Absent", "value"));
        assert!(!has_member_named(source, lang, "Store", "missing"));
    }
    for lang in [
        Language::Go,
        Language::TypeScript,
        Language::JavaScript,
        Language::Swift,
        Language::Cpp,
        Language::C,
    ] {
        for source in [
            "",
            "class Store",
            "class Store {",
            "struct Store {",
            "type Store struct",
            "type Store struct {",
        ] {
            assert!(!has_member_named(source, lang, "Store", "value"));
        }
    }
}

#[test]
fn extensions_are_case_sensitive_and_language_specific() {
    for (lang, extensions) in [
        (Language::Rust, vec!["rs"]),
        (Language::TypeScript, vec!["ts", "tsx"]),
        (Language::JavaScript, vec!["js", "jsx", "mjs", "cjs"]),
        (Language::Python, vec!["py"]),
        (Language::Go, vec!["go"]),
        (Language::Swift, vec!["swift"]),
        (Language::Cpp, vec!["cpp", "cc", "cxx", "hpp", "h"]),
        (Language::C, vec!["c", "h"]),
        (Language::Java, vec!["java"]),
    ] {
        for ext in extensions {
            assert!(language_matches_extension(
                lang,
                Path::new(&format!("source.{ext}"))
            ));
        }
        for path in ["source", "source.txt", "source.RS"] {
            assert!(!language_matches_extension(lang, Path::new(path)));
        }
    }
}
