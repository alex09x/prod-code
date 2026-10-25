/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Go and TypeScript declaration parsing: shapes, signatures and malformed input.

use super::super::mock::MethodSignature;
use super::parsers_go_ts::{parse_go_shape, parse_ts_shape};
use super::types::PolyglotShape;

fn method(name: &str, params: &[(&str, &str)], ret: Option<&str>) -> MethodSignature {
    MethodSignature {
        name: name.into(),
        params: params
            .iter()
            .map(|(n, t)| (n.to_string(), t.to_string()))
            .collect(),
        return_type: ret.map(str::to_string),
    }
}

fn record(fields: &[(&str, &str)]) -> PolyglotShape {
    PolyglotShape::Record(
        fields
            .iter()
            .map(|(n, t)| (n.to_string(), t.to_string()))
            .collect(),
    )
}

#[test]
fn go_struct_fields_embedded_types_and_comments() {
    let decl = "type Server struct {\n    // leading comment\n    Host, Name string `json:\"host\"`\n    *Config\n    sync.Mutex\n    Port int // trailing\n    Labels map[string]string\n    Callback func()\n}";
    assert_eq!(
        parse_go_shape(decl),
        Some(record(&[
            ("Host", "string"),
            ("Name", "string"),
            ("Config", "*Config"),
            ("Mutex", "sync.Mutex"),
            ("Port", "int"),
            ("Labels", "map[string]string"),
            ("Callback", "func()"),
        ]))
    );
}

#[test]
fn go_interface_method_signatures() {
    let decl = "type Store interface {\n    Get(key string) (value string, err error)\n    Put(key, value string) error\n    Close()\n    Do(int, string) (int, error)\n    Names() []string // note\n    Watch(ctx context.Context, opts ...Option) <-chan Event\n    Pair() (a, b int, error)\n    Broken(x int\n    Foo-Bar()\n    io.Reader\n    // only a comment\n}";
    let want = PolyglotShape::Interface {
        methods: vec![
            method("Get", &[("key", "string")], Some("string, error")),
            method(
                "Put",
                &[("key", "string"), ("value", "string")],
                Some("error"),
            ),
            method("Close", &[], None),
            method("Do", &[("", "int"), ("", "string")], Some("int, error")),
            method("Names", &[], Some("[]string")),
            method(
                "Watch",
                &[("ctx", "context.Context"), ("opts", "...Option")],
                Some("<-chan Event"),
            ),
            method("Pair", &[], Some("int, int, error")),
        ],
    };
    assert_eq!(parse_go_shape(decl), Some(want));
}

#[test]
fn go_empty_and_unrecognised_declarations() {
    let cases: &[(&str, Option<PolyglotShape>)] = &[
        ("type Empty struct {}", Some(record(&[]))),
        (
            "type Any interface {}",
            Some(PolyglotShape::Interface { methods: vec![] }),
        ),
        (
            "  \n type Padded struct { A int }  \n",
            Some(record(&[("A", "int")])),
        ),
        ("type Count int", None),
        ("type Open struct", None),
        ("type Fn func() { }", None),
        ("", None),
        ("} {", None),
    ];
    for (decl, want) in cases {
        assert_eq!(&parse_go_shape(decl), want, "{decl:?}");
    }
}

#[test]
fn ts_enum_variants() {
    let decl = "export enum Color {\n  Red = \"RED\", // note\n  Green,\n  Blue = 3,\n  \n}";
    assert_eq!(
        parse_ts_shape(decl),
        Some(PolyglotShape::Enum(vec![
            "Red".into(),
            "Green".into(),
            "Blue".into()
        ]))
    );
    assert_eq!(
        parse_ts_shape("enum Empty {}"),
        Some(PolyglotShape::Enum(vec![]))
    );
    assert_eq!(parse_ts_shape("enum Open"), None);
    assert_eq!(parse_ts_shape("enum X } {"), None);
}

#[test]
fn ts_interface_with_methods_and_properties() {
    let decl = "export interface Api {\n  readonly baseUrl: string;\n  timeout?: number;\n  fetch(path: string, opts?: Options): Promise<Response>;\n  async ping(): Promise<void>;\n  tags: Array<string>, // c\n  handler: (x: number) => void;\n}";
    let want = PolyglotShape::InterfaceWithFields {
        methods: vec![
            method(
                "fetch",
                &[("path", "string"), ("opts", "Options")],
                Some("Promise<Response>"),
            ),
            method("ping", &[], Some("Promise<void>")),
        ],
        fields: vec![
            ("baseUrl".into(), "string".into()),
            ("timeout".into(), "number".into()),
            ("tags".into(), "Array<string>".into()),
            ("handler".into(), "(x: number) => void".into()),
        ],
    };
    assert_eq!(parse_ts_shape(decl), Some(want));
}

#[test]
fn ts_shapes_by_content() {
    let cases: &[(&str, Option<PolyglotShape>)] = &[
        (
            "interface Cb { run(code: number): void; stop(): void }",
            Some(PolyglotShape::Interface {
                methods: vec![
                    method("run", &[("code", "number")], Some("void")),
                    method("stop", &[], Some("void")),
                ],
            }),
        ),
        (
            "type P = { a: string; b: number }",
            Some(record(&[("a", "string"), ("b", "number")])),
        ),
        (
            "class C { public name: string; $ref: string; [key: string]: any }",
            Some(record(&[("name", "string"), ("$ref", "string")])),
        ),
        ("interface X { broken(x: number }", Some(record(&[]))),
        ("interface X {}", Some(record(&[]))),
        ("type Alias = string", None),
        ("interface X } {", None),
    ];
    for (decl, want) in cases {
        assert_eq!(&parse_ts_shape(decl), want, "{decl:?}");
    }
}
