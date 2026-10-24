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

use super::{HierarchyRefactorResult, find_matching_brace, push_down_impl};
use std::net::SocketAddr;

fn sample_result(operation: &str) -> HierarchyRefactorResult {
    HierarchyRefactorResult {
        operation: operation.to_string(),
        source_class: "Dog".to_string(),
        target_classes: vec!["Animal".to_string()],
        members: vec!["bark".to_string(), "eat".to_string()],
        files_modified: vec!["a.py".to_string()],
        overlays: Vec::new(),
        diff: String::new(),
        applied: false,
        verified: false,
        diagnostics: Vec::new(),
    }
}

#[test]
fn render_formats_header_flags_and_omits_empty_sections() {
    let out = sample_result("pull_up").render(100);
    assert_eq!(
        out,
        "Pull Up — source: `Dog` -> targets: [\"Animal\"]\n\
         - members (2): bark, eat\n\
         - files modified (1): a.py\n\
         - applied: false\n\
         - verified: false\n"
    );
    let out = sample_result("push_down").render(100);
    assert!(out.starts_with("Push Down — source: `Dog`"));
}

#[test]
fn render_lists_diagnostics_and_truncates_long_diffs() {
    let mut r = sample_result("push_down");
    r.applied = true;
    r.verified = true;
    r.diagnostics = vec!["E1".to_string(), "E2".to_string()];
    r.diff = "0123456789".to_string();
    let out = r.render(4);
    assert!(out.contains("- applied: true\n- verified: true\n"));
    assert!(out.contains("- diagnostics (2):\n  • E1\n  • E2\n"));
    assert!(out.ends_with("\nDiff:\n```diff\n0123\n... [truncated]\n```\n"));
    let full = r.render(10);
    assert!(full.ends_with("\nDiff:\n```diff\n0123456789```\n"));
}

#[test]
fn find_matching_brace_table() {
    let cases: &[(&str, usize, Option<usize>)] = &[
        ("{ a }", 0, Some(4)),
        ("{ { } }", 0, Some(6)),
        ("{ { } }", 2, Some(4)),
        ("x { }", 0, None),
        ("{ a", 0, None),
        ("{ a }", 9, None),
        ("{ \"}\" }", 0, Some(6)),
        ("{ \"\\\"}\" }", 0, Some(8)),
        ("{ '}' }", 0, Some(6)),
        ("{ '\\'}' }", 0, Some(8)),
        ("{ `}` }", 0, Some(6)),
        ("{ `\\`}` }", 0, Some(8)),
        ("{ // }\n}", 0, Some(7)),
        ("{ // }", 0, None),
        ("{ /* } */ }", 0, Some(10)),
        ("{ /* } }", 0, None),
        ("{ / }", 0, Some(4)),
        ("{ \"unterminated }", 0, None),
    ];
    for (text, open, want) in cases {
        assert_eq!(find_matching_brace(text, *open), *want, "text {text:?}");
    }
}

const LOCAL: &str = "127.0.0.1:9";

fn push_down_error(
    files: &[(&str, &str)],
    file: &str,
    class: &str,
    targets: Option<&[String]>,
    members: &[&str],
    force: bool,
) -> String {
    let dir = tempfile::Builder::new().prefix("pp").tempdir().unwrap();
    for (name, text) in files {
        std::fs::write(dir.path().join(name), text).unwrap();
    }
    let members: Vec<String> = members.iter().map(|m| m.to_string()).collect();
    let remote: SocketAddr = LOCAL.parse().unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let err = rt
        .block_on(push_down_impl(
            remote,
            dir.path(),
            &dir.path().join(file),
            class,
            targets,
            &members,
            false,
            force,
            None,
        ))
        .unwrap_err();
    err.to_string()
        .replace(dir.path().to_str().unwrap(), "<dir>")
}

#[test]
fn push_down_refuses_before_touching_the_analyzer() {
    let animals = "class Animal:\n    def eat(self):\n        pass\n\nclass Dog(Animal):\n    def eat(self):\n        pass\n";
    let files = [("animals.py", animals)];
    let cat = vec!["Cat".to_string()];
    let cases: Vec<(String, &str)> = vec![
        (
            push_down_error(&files, "animals.py", "Animal", None, &[], false),
            "No members specified to push down",
        ),
        (
            push_down_error(&files, "missing.py", "Animal", None, &["eat"], false),
            "cannot read file <dir>/missing.py",
        ),
        (
            push_down_error(&files, "animals.py", "Nope", None, &["eat"], false),
            "class 'Nope' not found in <dir>/animals.py",
        ),
        (
            push_down_error(&files, "animals.py", "Animal", None, &["zip"], false),
            "member 'zip' not found in superclass 'Animal'",
        ),
        (
            push_down_error(&files, "animals.py", "Animal", Some(&cat), &["eat"], false),
            "specified target subclass 'Cat' not found as a subclass of 'Animal'",
        ),
        (
            push_down_error(&files, "animals.py", "Animal", None, &["eat"], false),
            "target subclass 'Dog' already defines member 'eat'",
        ),
        (
            push_down_error(
                &[("a.py", "class Lone:\n    def eat(self):\n        pass\n")],
                "a.py",
                "Lone",
                None,
                &["eat"],
                false,
            ),
            "no subclasses of 'Lone' found in workspace to push down to",
        ),
    ];
    for (got, want) in cases {
        assert_eq!(got, want);
    }
}
