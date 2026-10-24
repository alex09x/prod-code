/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Python, Rust, C/C++, Swift and Java declaration parsing through `parse_polyglot_shape`.

use super::super::mock::MethodSignature;
use super::parsers_others::parse_polyglot_shape;
use super::types::PolyglotShape;
use crate::parameter_object::Language;
use crate::parameter_object::Language::{C, Cpp, Java, JavaScript, Python, Rust, Swift};

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

fn check(language: Language, cases: &[(&str, Option<PolyglotShape>)]) {
    for (decl, want) in cases {
        assert_eq!(
            &parse_polyglot_shape(decl, language),
            want,
            "{language:?} {decl:?}"
        );
    }
}

#[test]
fn python_init_signature_and_protocol_methods() {
    check(
        Python,
        &[
            (
                "class Client:\n    def __init__(self, host: str, port: int = 8080, debug=False, name):\n        self.host = host",
                Some(record(&[
                    ("host", "str"),
                    ("port", "int"),
                    ("debug", ""),
                    ("name", ""),
                ])),
            ),
            (
                "class Repo(Protocol):\n    @abstractmethod\n    def get(self, key: str) -> Optional[str]: ...\n    def put(self, key: str, value) -> None:\n    def pairs(self) -> Tuple[int, ...]: ...\n    def __repr__(self) -> str:\n    def stop(self):\n    def broken",
                Some(PolyglotShape::Interface {
                    methods: vec![
                        method("get", &[("key", "str")], Some("Optional[str]")),
                        method("put", &[("key", "str"), ("value", "")], Some("None")),
                        method("pairs", &[], Some("Tuple[int, ...]")),
                        method("stop", &[], None),
                    ],
                }),
            ),
            (
                "class A:\n    x: int  # count\n    self.y: int\n    d = {\"a\": 1}\n    def f(self): pass",
                Some(record(&[("x", "int")])),
            ),
            ("", Some(record(&[]))),
            ("pass", Some(record(&[]))),
        ],
    );
}

#[test]
fn rust_trait_methods_and_type_declarations() {
    let trait_decl = "pub trait Store: Send {\n    // docs\n    fn get(&self, key: &str) -> Option<String>;\n    fn put(&mut self, key: String, value: String);\n    fn consume(self) -> u8;\n    fn broken(x: u8;\n    type Item;\n}";
    check(
        Rust,
        &[
            (
                trait_decl,
                Some(PolyglotShape::Interface {
                    methods: vec![
                        method(
                            "get",
                            &[("&self", ""), ("key", "&str")],
                            Some("Option<String>"),
                        ),
                        method(
                            "put",
                            &[("&mut self", ""), ("key", "String"), ("value", "String")],
                            None,
                        ),
                        method("consume", &[("self", "")], Some("u8")),
                    ],
                }),
            ),
            (
                "trait Empty {}",
                Some(PolyglotShape::Interface { methods: vec![] }),
            ),
            ("trait Open;", None),
            ("trait T } {", None),
            (
                "pub struct A { pub x: i32, y: String }",
                Some(record(&[("x", "i32"), ("y", "String")])),
            ),
            (
                "struct P(pub u8, String);",
                Some(PolyglotShape::Tuple(vec!["u8".into(), "String".into()])),
            ),
            ("struct U;", Some(PolyglotShape::Unit)),
            (
                "enum E {\n    // c\n    A,\n    B(u8),\n    #[x]\n    C { x: i32 },\n}",
                Some(PolyglotShape::Enum(vec![
                    "A".into(),
                    "B".into(),
                    "C".into(),
                ])),
            ),
            ("struct X {", None),
            ("struct X } {", None),
        ],
    );
}

#[test]
fn cpp_fields_virtual_methods_and_comments() {
    let class = "class Service {\npublic:\n    virtual void run(int code) = 0;\n    virtual std::string name() const = 0;\n    virtual ~Service() = default;\nprivate:\n    int port = 0;\n    std::string host;\n};";
    let iface = "class Runner {\npublic:\n    virtual void run(int code) = 0;\n    virtual int add(int, int) = 0;\n};";
    let commented =
        "struct A {\n    int x; // the x\n    int y;\n    # not a field\n    float z;\n};";
    for lang in [Cpp, C] {
        check(
            lang,
            &[
                (
                    class,
                    Some(record(&[("port", "int"), ("host", "std::string")])),
                ),
                (
                    iface,
                    Some(PolyglotShape::Interface {
                        methods: vec![
                            method("run", &[("code", "int")], Some("void")),
                            method("add", &[("", "int"), ("", "int")], Some("int")),
                        ],
                    }),
                ),
                (
                    commented,
                    Some(record(&[("x", "int"), ("y", "int"), ("z", "float")])),
                ),
                ("struct E {};", Some(record(&[]))),
                ("struct Fwd;", None),
                ("struct B } {", None),
            ],
        );
    }
}

#[test]
fn java_fields_drop_modifiers() {
    check(
        Java,
        &[(
            "public class User {\n    private final String name;\n    protected int age = 3;\n}",
            Some(record(&[("name", "String"), ("age", "int")])),
        )],
    );
}

#[test]
fn swift_protocol_and_struct() {
    check(
        Swift,
        &[
            (
                "protocol Greeter {\n    func greet(name: String, times: Int) -> String\n    func reset()\n    var title: String { get }\n    func broken(x: Int\n}",
                Some(PolyglotShape::Interface {
                    methods: vec![
                        method(
                            "greet",
                            &[("name", "String"), ("times", "Int")],
                            Some("String"),
                        ),
                        method("reset", &[], None),
                    ],
                }),
            ),
            (
                "struct Point: Codable {\n    var x: Double\n    let y: Double = 0   // c\n    var label: String? = nil\n    var bad.name: Int\n    static let shared = Point()\n    func f() {}\n}",
                Some(record(&[
                    ("x", "Double"),
                    ("y", "Double"),
                    ("label", "String?"),
                ])),
            ),
            ("struct Empty {}", Some(record(&[]))),
            ("struct Open", None),
            ("struct B } {", None),
        ],
    );
}

#[test]
fn javascript_dispatches_to_the_typescript_parser() {
    check(
        JavaScript,
        &[(
            "interface A { a: string }",
            Some(record(&[("a", "string")])),
        )],
    );
}
