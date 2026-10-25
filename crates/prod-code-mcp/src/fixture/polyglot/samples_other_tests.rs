/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Exact sample values for Python, Rust, C/C++, Swift and Java types.

use super::samples::{inner_of_rust, sample_value_for_type};
use crate::parameter_object::Language;
use crate::parameter_object::Language::{C, Cpp, Java, Python, Rust, Swift};

type Case = (
    Language,
    &'static str,
    Option<&'static str>,
    bool,
    &'static str,
);

fn check(cases: &[Case]) {
    for (lang, ty, field, randomized, want) in cases {
        let got = sample_value_for_type(*lang, ty, *field, *randomized);
        assert_eq!(
            &got, want,
            "{lang:?} type {ty:?} field {field:?} randomized={randomized}"
        );
    }
}

#[test]
fn python_values() {
    check(&[
        (Python, "", None, false, "None"),
        (Python, "  ", Some("x"), true, "None"),
        (Python, "bool", None, false, "False"),
        (Python, "int", None, false, "0"),
        (Python, "float", None, false, "0.0"),
        (Python, "str", None, false, "\"\""),
        (Python, "None", None, false, "None"),
        (Python, "list[str]", None, false, "[]"),
        (Python, "List[int]", None, false, "[]"),
        (Python, "dict[str, int]", None, false, "{}"),
        (Python, "Dict[str,int]", None, false, "{}"),
        (Python, "set[int]", None, false, "set()"),
        (Python, "Set[int]", None, false, "set()"),
        (Python, "Optional[str]", None, false, "None"),
        (Python, "Settings", None, false, "Settings()"),
        (Python, "Listener", None, false, "Listener()"),
        (Python, "Dictionary", None, false, "Dictionary()"),
        (Python, "Foo", None, false, "Foo()"),
        (Python, "bool", None, true, "True"),
        (Python, "str", Some("user_id"), true, "\"id_9823\""),
        (Python, "str", Some("Email"), true, "\"user@example.com\""),
        (Python, "str", Some("name"), true, "\"test_sample_name\""),
        (Python, "str", Some("host"), true, "\"sample_host\""),
        (Python, "str", None, true, "\"sample_val\""),
        (Python, "int", Some("port"), true, "8080"),
        (Python, "int", None, true, "42"),
        (Python, "float", Some("price"), true, "99.95"),
        (Python, "float", None, true, "3.14"),
        (
            Python,
            "list[str]",
            None,
            true,
            "[\"sample_1\", \"sample_2\"]",
        ),
        (Python, "List[int]", None, true, "[1, 2, 3]"),
        (Python, "list[float]", None, true, "[]"),
        (Python, "List", None, true, "[]"),
        (
            Python,
            "dict[str, int]",
            None,
            true,
            "{\"key\": \"sample_val\"}",
        ),
        (Python, "set[int]", None, true, "set()"),
        (Python, "Optional[int]", None, true, "None"),
        (Python, "None", None, true, "None"),
        (Python, "Settings", None, true, "Settings()"),
    ]);
}

#[test]
fn rust_values() {
    check(&[
        (Rust, "bool", None, false, "false"),
        (Rust, "String", None, false, "String::new()"),
        (Rust, "&str", None, false, "\"\""),
        (Rust, "u32", None, false, "0"),
        (Rust, "Vec<u8>", None, false, "Vec::new()"),
        (Rust, "Option<String>", None, false, "None"),
        (Rust, "Config", None, false, "Default::default()"),
        (Rust, "bool", None, true, "true"),
        (
            Rust,
            "String",
            Some("host"),
            true,
            "\"sample_host\".to_string()",
        ),
        (Rust, "String", None, true, "\"sample_val\".to_string()"),
        (Rust, "&str", Some("label"), true, "\"sample_label\""),
        (Rust, "str", None, true, "\"sample_val\""),
        (Rust, "u16", Some("Port"), true, "8080"),
        (Rust, "u64", Some("timeout_ms"), true, "30"),
        (Rust, "usize", None, true, "42"),
        (Rust, "i128", None, true, "42"),
        (Rust, "f32", None, true, "3.14"),
        (Rust, "f64", None, true, "3.14"),
        (
            Rust,
            "Duration",
            None,
            true,
            "std::time::Duration::from_secs(30)",
        ),
        (
            Rust,
            "SocketAddr",
            None,
            true,
            "\"127.0.0.1:8080\".parse().unwrap()",
        ),
        (
            Rust,
            "Vec<String>",
            None,
            true,
            "vec![\"sample_val\".to_string()]",
        ),
        (Rust, "Vec<u8>", None, true, "vec![42]"),
        (
            Rust,
            "Vec<u8",
            None,
            true,
            "vec![\"sample_val\".to_string()]",
        ),
        (Rust, "Vec<>", None, true, "vec![Default::default()]"),
        (Rust, "Option<i32>", None, true, "Some(42)"),
        (Rust, "Option<Vec<u8>>", None, true, "Some(vec![42])"),
        (Rust, "std::vec::Vec<u8>", None, true, "Vec::new()"),
        (Rust, "PathBuf", None, true, "std::path::PathBuf::new()"),
        (Rust, "char", None, true, "'a'"),
        (Rust, "Config", None, true, "Default::default()"),
    ]);
}

#[test]
fn inner_of_rust_extracts_only_the_named_wrapper() {
    let cases: &[(&str, &str, Option<&str>)] = &[
        ("Vec<u8>", "Vec", Some("u8")),
        ("std::vec::Vec<u8>", "Vec", Some("u8")),
        (" Vec< String > ", "Vec", Some("String")),
        ("HashMap<K, V>", "HashMap", Some("K, V")),
        ("Box<Vec<u8>>", "Box", Some("Vec<u8>")),
        ("Option<u8>", "Vec", None),
        ("Vec", "Vec", None),
        ("Vec<u8", "Vec", None),
    ];
    for (ty, wrapper, want) in cases {
        assert_eq!(inner_of_rust(ty, wrapper), *want, "{ty:?} as {wrapper:?}");
    }
}

#[test]
fn c_and_cpp_values() {
    for lang in [Cpp, C] {
        check(&[
            (lang, "bool", None, false, "false"),
            (lang, "size_t", None, false, "0"),
            (lang, "int64_t", None, false, "0"),
            (lang, "double", None, false, "0.0"),
            (lang, "std::string", None, false, "\"\""),
            (lang, "string", None, false, "\"\""),
            (lang, "Widget", None, false, "{}"),
            (lang, "bool", None, true, "true"),
            (lang, "std::string", Some("label"), true, "\"sample_label\""),
            (lang, "string", None, true, "\"sample_val\""),
            (lang, "int", Some("port"), true, "8080"),
            (lang, "size_t", None, true, "42"),
            (lang, "double", None, true, "3.14"),
            (lang, "Widget", None, true, "{}"),
        ]);
    }
}

#[test]
fn swift_values() {
    check(&[
        (Swift, "Bool", None, false, "false"),
        (Swift, "UInt", None, false, "0"),
        (Swift, "Double", None, false, "0.0"),
        (Swift, "String", None, false, "\"\""),
        (Swift, "Int?", None, false, "nil"),
        (Swift, "[String]", None, false, "[]"),
        (Swift, "[String: Int]", None, false, "[:]"),
        (Swift, "Point", None, false, "Point()"),
        (Swift, "Bool", None, true, "true"),
        (Swift, "String", Some("title"), true, "\"sample_title\""),
        (Swift, "String", None, true, "\"sample_val\""),
        (Swift, "Int", Some("port"), true, "8080"),
        (Swift, "Int64", None, true, "42"),
        (Swift, "Float", None, true, "3.14"),
        (Swift, "Int?", None, true, "nil"),
        (Swift, "[String]", None, true, "[String]()"),
        (Swift, "Point", None, true, "Point()"),
    ]);
}

#[test]
fn java_values() {
    check(&[
        (Java, "boolean", None, false, "false"),
        (Java, "String", None, false, "\"\""),
        (Java, "short", None, false, "0"),
        (Java, "float", None, false, "0.0f"),
        (Java, "double", None, false, "0.0"),
        (Java, "char", None, false, "'\\0'"),
        (
            Java,
            "List<String>",
            None,
            false,
            "java.util.Collections.emptyList()",
        ),
        (
            Java,
            "java.util.List<String>",
            None,
            false,
            "java.util.Collections.emptyList()",
        ),
        (
            Java,
            "Map<String, Integer>",
            None,
            false,
            "java.util.Collections.emptyMap()",
        ),
        (
            Java,
            "java.util.Map<String, Integer>",
            None,
            false,
            "java.util.Collections.emptyMap()",
        ),
        (
            Java,
            "Set<String>",
            None,
            false,
            "java.util.Collections.emptySet()",
        ),
        (
            Java,
            "java.util.Set<String>",
            None,
            false,
            "java.util.Collections.emptySet()",
        ),
        (Java, "User", None, false, "null"),
        (Java, "boolean", None, true, "true"),
        (Java, "String", Some("Host"), true, "\"test_host\""),
        (Java, "String", None, true, "\"test_\""),
        (Java, "int", None, true, "42"),
        (Java, "long", None, true, "42"),
        (Java, "byte", None, true, "42"),
        (Java, "short", None, true, "42"),
        (Java, "double", None, true, "3.14"),
        (Java, "float", None, true, "3.14f"),
        (Java, "User", None, true, "null"),
    ]);
}
