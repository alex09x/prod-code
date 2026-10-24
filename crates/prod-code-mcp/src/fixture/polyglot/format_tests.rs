/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Rendering of fixture literals and mocks for every shape and language.

use super::super::mock::{MethodSignature, generate_mock};
use super::format::format_polyglot_fixture;
use super::types::PolyglotShape;
use crate::parameter_object::Language;
use crate::parameter_object::Language::{
    C, Cpp, Go, Java, JavaScript, Python, Rust, Swift, TypeScript,
};

const ALL: [Language; 9] = [
    Go, TypeScript, JavaScript, Python, Rust, Cpp, C, Swift, Java,
];

fn record(host: &str, port: &str) -> PolyglotShape {
    PolyglotShape::Record(vec![
        ("host".into(), host.into()),
        ("port".into(), port.into()),
    ])
}

fn getter() -> MethodSignature {
    MethodSignature {
        name: "get".into(),
        params: vec![("key".into(), "string".into())],
        return_type: Some("string".into()),
    }
}

#[test]
fn record_literal_per_language() {
    // (language, host type, port type, value, snippet prefix, snippet suffix)
    let cases: &[(Language, &str, &str, &str, &str, &str)] = &[
        (
            Go,
            "string",
            "int",
            "AppConfig{\n    host: \"\",\n    port: 0,\n}",
            "var appConfig = ",
            "",
        ),
        (
            TypeScript,
            "string",
            "number",
            "{\n    host: \"\",\n    port: 0,\n}",
            "const appConfig: AppConfig = ",
            ";",
        ),
        (
            JavaScript,
            "string",
            "number",
            "{\n    host: \"\",\n    port: 0,\n}",
            "const appConfig: AppConfig = ",
            ";",
        ),
        (
            Python,
            "str",
            "int",
            "AppConfig(\n    host=\"\",\n    port=0,\n)",
            "app_config = ",
            "",
        ),
        (
            Rust,
            "String",
            "u16",
            "AppConfig {\n    host: String::new(),\n    port: 0,\n}",
            "let app_config = ",
            ";",
        ),
        (
            Cpp,
            "std::string",
            "int",
            "AppConfig{\n    .host = \"\",\n    .port = 0,\n}",
            "auto app_config = ",
            ";",
        ),
        (
            C,
            "std::string",
            "int",
            "AppConfig{\n    .host = \"\",\n    .port = 0,\n}",
            "auto app_config = ",
            ";",
        ),
        (
            Swift,
            "String",
            "Int",
            "AppConfig(\n    host: \"\",\n    port: 0,\n)",
            "let appConfig = ",
            "",
        ),
        (
            Java,
            "String",
            "int",
            "new AppConfig(\"\", 0)",
            "AppConfig appConfig = ",
            ";",
        ),
    ];
    for (lang, host, port, val, pre, post) in cases {
        let got = format_polyglot_fixture(*lang, "AppConfig", &record(host, port), false, false);
        assert_eq!(
            got,
            (val.to_string(), format!("{pre}{val}{post}")),
            "{lang:?}"
        );
    }
}

#[test]
fn randomized_record_uses_field_names() {
    let got = format_polyglot_fixture(Rust, "AppConfig", &record("String", "u16"), true, false);
    assert_eq!(
        got.0,
        "AppConfig {\n    host: \"sample_host\".to_string(),\n    port: 8080,\n}"
    );
    let got = format_polyglot_fixture(Java, "AppConfig", &record("String", "int"), true, false);
    assert_eq!(
        got,
        (
            "new AppConfig(\"test_host\", 42)".to_string(),
            "AppConfig appConfig = new AppConfig(\"test_host\", 42);".to_string()
        )
    );
}

#[test]
fn tuple_and_unit_shapes() {
    let tuple = PolyglotShape::Tuple(vec!["i32".into(), "String".into()]);
    let got = format_polyglot_fixture(Rust, "Pair", &tuple, false, false);
    assert_eq!(
        got,
        (
            "Pair(0, String::new())".to_string(),
            "let pair = Pair(0, String::new());".to_string()
        )
    );
    let got = format_polyglot_fixture(Rust, "Pair", &tuple, true, false);
    assert_eq!(got.0, "Pair(42, \"sample_val\".to_string())");
    let got = format_polyglot_fixture(Rust, "Marker", &PolyglotShape::Unit, false, false);
    assert_eq!(
        got,
        ("Marker".to_string(), "let marker = Marker;".to_string())
    );
}

#[test]
fn enum_picks_first_or_second_variant() {
    let two = PolyglotShape::Enum(vec!["Alpha".into(), "Beta".into()]);
    let one = PolyglotShape::Enum(vec!["Alpha".into()]);
    let none = PolyglotShape::Enum(Vec::new());
    // (language, shape, randomized, expected value)
    let cases: &[(Language, &PolyglotShape, bool, &str)] = &[
        (Rust, &two, false, "Mode::Alpha"),
        (Rust, &two, true, "Mode::Beta"),
        (Rust, &one, true, "Mode::Alpha"),
        (Rust, &none, false, "Mode::Default"),
        (Rust, &none, true, "Mode::Default"),
        (Swift, &two, false, "Mode.Alpha"),
        (TypeScript, &two, false, "Mode.Alpha"),
        (Go, &two, false, "Alpha"),
        (Go, &two, true, "Beta"),
        (Python, &two, false, "Mode.Alpha"),
        (Java, &two, true, "Mode.Beta"),
    ];
    for (lang, shape, randomized, want) in cases {
        let (val, snippet) = format_polyglot_fixture(*lang, "Mode", shape, *randomized, false);
        assert_eq!(val, *want, "{lang:?} randomized={randomized}");
        assert_eq!(snippet, format!("let mode = {want};"), "{lang:?}");
    }
}

#[test]
fn interfaces_render_as_mocks() {
    let methods = vec![getter()];
    let fields = vec![("limit".to_string(), "int".to_string())];
    let iface = PolyglotShape::Interface {
        methods: methods.clone(),
    };
    let mixed = PolyglotShape::InterfaceWithFields {
        methods: methods.clone(),
        fields: fields.clone(),
    };
    for lang in ALL {
        let want = generate_mock(lang, "Store", &methods, &[]);
        assert!(!want.is_empty(), "{lang:?}");
        assert_eq!(
            format_polyglot_fixture(lang, "Store", &iface, false, false),
            (want.clone(), want),
            "{lang:?} interface"
        );
        let want = generate_mock(lang, "Store", &methods, &fields);
        assert_eq!(
            format_polyglot_fixture(lang, "Store", &mixed, true, false),
            (want.clone(), want),
            "{lang:?} interface with fields"
        );
    }
}

#[test]
fn mock_flag_routes_every_shape_to_the_mock_generator() {
    let methods = vec![getter()];
    let fields = vec![("limit".to_string(), "int".to_string())];
    let no_m: &[MethodSignature] = &[];
    let no_f: &[(String, String)] = &[];
    // (shape, methods and fields the mock generator must receive)
    #[allow(clippy::type_complexity)]
    let cases: Vec<(PolyglotShape, &[MethodSignature], &[(String, String)])> = vec![
        (
            PolyglotShape::Interface {
                methods: methods.clone(),
            },
            methods.as_slice(),
            no_f,
        ),
        (
            PolyglotShape::InterfaceWithFields {
                methods: methods.clone(),
                fields: fields.clone(),
            },
            methods.as_slice(),
            fields.as_slice(),
        ),
        (
            PolyglotShape::Record(fields.clone()),
            no_m,
            fields.as_slice(),
        ),
        (PolyglotShape::Unit, no_m, no_f),
        (PolyglotShape::Tuple(vec!["i32".into()]), no_m, no_f),
        (PolyglotShape::Enum(vec!["A".into()]), no_m, no_f),
    ];
    for lang in [Go, Rust, Python] {
        for (shape, m, f) in &cases {
            let want = generate_mock(lang, "Store", m, f);
            let got = format_polyglot_fixture(lang, "Store", shape, true, true);
            assert_eq!(got, (want.clone(), want), "{lang:?} {shape:?}");
        }
    }
}
