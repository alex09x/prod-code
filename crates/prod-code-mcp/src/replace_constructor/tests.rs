/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::codegen::{generate_builder_code, generate_factory_code};
use super::parse::{
    parse_go_struct_decl, parse_python_struct_decl, parse_rust_struct_decl, parse_ts_struct_decl,
};
use super::rewrite::rewrite_instantiation;
use super::sites::find_rust_instantiations;
use super::types::ReplaceMode;

#[test]
fn parses_rust_struct_fields_and_generates_factory_and_builder() {
    let rust_code = r#"
pub struct User {
    pub name: String,
    pub age: u32,
    email: Option<String>,
}
"#;
    let decl = parse_rust_struct_decl(rust_code, "User").unwrap();
    assert_eq!(decl.name, "User");
    assert!(decl.is_pub);
    assert_eq!(decl.fields.len(), 3);
    assert_eq!(decl.fields[0].name, "name");
    assert_eq!(decl.fields[0].ty, "String");
    assert_eq!(decl.fields[1].name, "age");
    assert_eq!(decl.fields[1].ty, "u32");
    assert_eq!(decl.fields[2].name, "email");
    assert_eq!(decl.fields[2].ty, "Option<String>");

    let factory = generate_factory_code(&decl, "new");
    assert!(factory.contains("pub fn new(name: String, age: u32, email: Option<String>) -> Self"));
    assert!(
        factory
            .contains("Self {\n            name,\n            age,\n            email,\n        }")
    );

    let builder = generate_builder_code(&decl, "UserBuilder");
    assert!(builder.contains("pub struct UserBuilder"));
    assert!(builder.contains("pub fn name(mut self, value: String) -> Self"));
    assert!(builder.contains("pub fn age(mut self, value: u32) -> Self"));
    assert!(builder.contains("pub fn build(self) -> User"));
    assert!(builder.contains("pub fn builder() -> UserBuilder"));
}

#[test]
fn finds_and_rewrites_rust_instantiations() {
    let code = r#"
struct Config {
    pub host: String,
    pub port: u16,
}

fn run() {
    let c1 = Config { host: "127.0.0.1".into(), port: 8080 };
    let c2 = Config { port: 9000, host: "0.0.0.0".into() };
    let host = "localhost".into();
    let port = 3000;
    let c3 = Config { host, port };
}
"#;
    let decl = parse_rust_struct_decl(code, "Config").unwrap();
    let (sites, blocked) = find_rust_instantiations(code, "Config", decl.decl_start, decl.decl_end);
    assert!(blocked.is_empty());
    assert_eq!(sites.len(), 3);

    // Factory rewrite
    let r1 = rewrite_instantiation(&sites[0], &decl, ReplaceMode::Factory, "new").unwrap();
    assert_eq!(r1, "Config::new(\"127.0.0.1\".into(), 8080)");

    // Reordered fields mapped to declared parameter order:
    let r2 = rewrite_instantiation(&sites[1], &decl, ReplaceMode::Factory, "new").unwrap();
    assert_eq!(r2, "Config::new(\"0.0.0.0\".into(), 9000)");

    // Shorthand fields:
    let r3 = rewrite_instantiation(&sites[2], &decl, ReplaceMode::Factory, "new").unwrap();
    assert_eq!(r3, "Config::new(host, port)");

    // Builder rewrite
    let b1 =
        rewrite_instantiation(&sites[0], &decl, ReplaceMode::Builder, "ConfigBuilder").unwrap();
    assert_eq!(
        b1,
        "Config::builder().host(\"127.0.0.1\".into()).port(8080).build()"
    );
}

#[test]
fn detects_and_blocks_struct_update_syntax() {
    let code = r#"
struct Point {
    x: i32,
    y: i32,
}

fn foo() {
    let base = Point { x: 1, y: 2 };
    let p2 = Point { x: 5, ..base };
}
"#;
    let decl = parse_rust_struct_decl(code, "Point").unwrap();
    let (sites, blocked) = find_rust_instantiations(code, "Point", decl.decl_start, decl.decl_end);
    assert_eq!(sites.len(), 1);
    assert_eq!(blocked.len(), 1);
    assert!(blocked[0].contains("struct update syntax `..`"));
}

#[test]
fn polyglot_go_parsing_and_generation() {
    let go_code = r#"
package models

type Service struct {
    Name    string
    Timeout int
}
"#;
    let decl = parse_go_struct_decl(go_code, "Service").unwrap();
    assert_eq!(decl.fields.len(), 2);
    assert_eq!(decl.fields[0].name, "Name");
    assert_eq!(decl.fields[1].name, "Timeout");

    let factory = generate_factory_code(&decl, "NewService");
    assert!(factory.contains("func NewService(Name string, Timeout int) *Service"));
    assert!(factory.contains("return &Service{"));

    let builder = generate_builder_code(&decl, "ServiceBuilder");
    assert!(builder.contains("type ServiceBuilder struct"));
    assert!(builder.contains("func (b *ServiceBuilder) Name(v string) *ServiceBuilder"));
    assert!(builder.contains("func (b *ServiceBuilder) Build() *Service"));
}

#[test]
fn polyglot_ts_parsing_and_generation() {
    let ts_code = r#"
export class Person {
    name: string;
    age: number;
    constructor(name: string, age: number) {
        this.name = name;
        this.age = age;
    }
}
"#;
    let decl = parse_ts_struct_decl(ts_code, "Person", "typescript").unwrap();
    assert_eq!(decl.fields.len(), 2);
    assert_eq!(decl.fields[0].name, "name");
    assert_eq!(decl.fields[1].name, "age");

    let factory = generate_factory_code(&decl, "create");
    assert!(factory.contains("static create(name: string, age: number): Person"));

    let builder = generate_builder_code(&decl, "PersonBuilder");
    assert!(builder.contains("export class PersonBuilder"));
    assert!(builder.contains("name(value: string): this"));
    assert!(builder.contains("build(): Person"));
}

#[test]
fn polyglot_python_parsing_and_generation() {
    let py_code = r#"
class Car:
    def __init__(self, make: str, model: str):
        self.make = make
        self.model = model
"#;
    let decl = parse_python_struct_decl(py_code, "Car").unwrap();
    assert_eq!(decl.fields.len(), 2);
    assert_eq!(decl.fields[0].name, "make");
    assert_eq!(decl.fields[1].name, "model");

    let factory = generate_factory_code(&decl, "create");
    assert!(factory.contains("@classmethod"));
    assert!(factory.contains("def create(cls, make: str, model: str) -> \"Car\":"));

    let builder = generate_builder_code(&decl, "CarBuilder");
    assert!(builder.contains("class CarBuilder:"));
    assert!(builder.contains("def make(self, value):"));
    assert!(builder.contains("def build(self) -> \"Car\":"));
}
