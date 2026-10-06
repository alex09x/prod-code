/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::format::strip_override_modifiers;
use super::languages::{
    parse_cpp_classes, parse_python_classes, parse_rust_traits, parse_swift_classes,
    parse_ts_classes,
};
use std::path::Path;

#[test]
fn python_class_parsing_and_pull_up() {
    let py_code = r#"
class Animal:
    pass

class Dog(Animal):
    def bark(self):
        print("woof")

    def eat(self):
        print("eating")
"#;
    let classes = parse_python_classes(py_code, Path::new("test.py"));
    assert_eq!(classes.len(), 2);
    assert_eq!(classes[0].name, "Animal");
    assert_eq!(classes[1].name, "Dog");
    assert_eq!(classes[1].super_names, vec!["Animal"]);
    assert_eq!(classes[1].members.len(), 2);
    assert_eq!(classes[1].members[0].name, "bark");
    assert_eq!(classes[1].members[1].name, "eat");
}

#[test]
fn ts_class_parsing_and_override_stripping() {
    let ts_code = r#"
export class Animal {
    name: string;
}

export class Dog extends Animal {
    override bark(): string {
        return "woof";
    }
}
"#;
    let classes = parse_ts_classes(ts_code, Path::new("test.ts"), "typescript");
    assert_eq!(classes.len(), 2);
    assert_eq!(classes[0].name, "Animal");
    assert_eq!(classes[1].name, "Dog");
    assert_eq!(classes[1].super_names, vec!["Animal"]);
    assert_eq!(classes[1].members.len(), 1);
    assert_eq!(classes[1].members[0].name, "bark");
    assert!(classes[1].members[0].is_override);

    let stripped = strip_override_modifiers(&classes[1].members[0].full_text, "typescript");
    assert!(!stripped.contains("override"));
    assert!(stripped.contains("bark(): string"));
}

#[test]
fn cpp_class_parsing() {
    let cpp_code = r#"
class Base {
public:
    int id;
};

class Derived : public Base {
public:
    void greet() override {
        std::cout << "hello\n";
    }
};
"#;
    let classes = parse_cpp_classes(cpp_code, Path::new("test.cpp"));
    assert_eq!(classes.len(), 2);
    assert_eq!(classes[0].name, "Base");
    assert_eq!(classes[1].name, "Derived");
    assert_eq!(classes[1].super_names, vec!["Base"]);
    assert_eq!(classes[1].members.len(), 1);
    assert_eq!(classes[1].members[0].name, "greet");
    assert!(classes[1].members[0].is_override);
}

#[test]
fn swift_class_parsing() {
    let swift_code = r#"
class Animal {
    var name: String
}

class Dog: Animal {
    override func speak() {
        print("woof")
    }
}
"#;
    let classes = parse_swift_classes(swift_code, Path::new("test.swift"));
    assert_eq!(classes.len(), 2);
    assert_eq!(classes[0].name, "Animal");
    assert_eq!(classes[1].name, "Dog");
    assert_eq!(classes[1].super_names, vec!["Animal"]);
    assert_eq!(classes[1].members.len(), 1);
    assert_eq!(classes[1].members[0].name, "speak");
    assert!(classes[1].members[0].is_override);
}

#[test]
fn rust_trait_parsing() {
    let rust_code = r#"
pub trait SuperTrait {
    fn base_fn(&self);
}

pub trait SubTrait: SuperTrait {
    fn sub_fn(&self);
    type Item;
}
"#;
    let traits = parse_rust_traits(rust_code, Path::new("test.rs"));
    assert_eq!(traits.len(), 2);
    assert_eq!(traits[0].name, "SuperTrait");
    assert_eq!(traits[1].name, "SubTrait");
    assert_eq!(traits[1].super_names, vec!["SuperTrait"]);
    assert_eq!(traits[1].members.len(), 2);
    assert_eq!(traits[1].members[0].name, "sub_fn");
    assert_eq!(traits[1].members[1].name, "Item");
}
