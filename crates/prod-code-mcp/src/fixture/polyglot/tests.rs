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
use crate::parameter_object::Language;

#[test]
fn parses_go_struct_shape() {
    let decl = "type ServerConfig struct {\n    Host string `json:\"host\"`\n    Port int `json:\"port\"`\n    TLS bool\n}";
    let shape = parse_polyglot_shape(decl, Language::Go).expect("go struct shape");
    assert_eq!(
        shape,
        PolyglotShape::Record(vec![
            ("Host".into(), "string".into()),
            ("Port".into(), "int".into()),
            ("TLS".into(), "bool".into()),
        ])
    );
}

#[test]
fn parses_go_interface_shape() {
    let decl =
        "type Reader interface {\n    Read(p []byte) (n int, err error)\n    Close() error\n}";
    let shape = parse_polyglot_shape(decl, Language::Go).expect("go interface shape");
    match shape {
        PolyglotShape::Interface { methods } => {
            assert_eq!(methods.len(), 2);
            assert_eq!(methods[0].name, "Read");
            assert_eq!(methods[0].params, vec![("p".into(), "[]byte".into())]);
            assert_eq!(methods[0].return_type, Some("int, error".into()));
            assert_eq!(methods[1].name, "Close");
            assert_eq!(methods[1].return_type, Some("error".into()));
        }
        _ => panic!("expected interface"),
    }
}

#[test]
fn parses_ts_interface_shape() {
    let decl = "export interface UserProfile {\n    id: string;\n    displayName: string;\n    age?: number;\n    active: boolean;\n}";
    let shape = parse_polyglot_shape(decl, Language::TypeScript).expect("ts interface shape");
    assert_eq!(
        shape,
        PolyglotShape::Record(vec![
            ("id".into(), "string".into()),
            ("displayName".into(), "string".into()),
            ("age".into(), "number".into()),
            ("active".into(), "boolean".into()),
        ])
    );
}

#[test]
fn parses_python_dataclass_shape() {
    let decl =
        "@dataclass\nclass Config:\n    host: str\n    port: int = 8080\n    enabled: bool = True";
    let shape = parse_polyglot_shape(decl, Language::Python).expect("python shape");
    assert_eq!(
        shape,
        PolyglotShape::Record(vec![
            ("host".into(), "str".into()),
            ("port".into(), "int".into()),
            ("enabled".into(), "bool".into()),
        ])
    );
}

#[test]
fn generates_randomized_dummy_data() {
    let val = sample_value_for_type(Language::Go, "string", Some("email"), true);
    assert_eq!(val, "\"user@example.com\"");
    let port = sample_value_for_type(Language::Go, "int", Some("port"), true);
    assert_eq!(port, "8080");
    let active = sample_value_for_type(Language::TypeScript, "boolean", None, true);
    assert_eq!(active, "true");
}

#[test]
fn formats_go_fixture_literal() {
    let shape = PolyglotShape::Record(vec![
        ("Host".into(), "string".into()),
        ("Port".into(), "int".into()),
    ]);
    let (val, snippet) = format_polyglot_fixture(Language::Go, "Config", &shape, false, false);
    assert!(val.contains("Host: \"\","));
    assert!(val.contains("Port: 0,"));
    assert!(snippet.contains("var config = Config{"));
}

#[test]
fn formats_ts_fixture_literal() {
    let shape = PolyglotShape::Record(vec![
        ("id".into(), "string".into()),
        ("active".into(), "boolean".into()),
    ]);
    let (val, snippet) = format_polyglot_fixture(Language::TypeScript, "User", &shape, true, false);
    assert!(val.contains("id: \"id_9823\","));
    assert!(val.contains("active: true,"));
    assert!(snippet.contains("const user: User = {"));
}
