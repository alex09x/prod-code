/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::support::Case;
use prod_code_mcp::{
    pull_push::{ClassDecl, parse_classes_in_text},
    replace_inheritance::languages::{transform_swift, transform_typescript},
};
use std::path::Path;

type Transform =
    fn(&str, &ClassDecl, &str, &str, &[String], Option<&ClassDecl>) -> anyhow::Result<String>;

#[test]
fn replacement_strips_each_typescript_and_swift_declaration_only() {
    for (language, header, base_members, first, second, transform) in [
        (
            "typescript",
            "class Dog extends Pet",
            "    speak() { return \"base\"; }\n    other() { return \"base\"; }",
            "public override /* override final */ speak()",
            "public override /* override final */ other()",
            transform_typescript as Transform,
        ),
        (
            "swift",
            "class Dog: Pet",
            "    func speak() -> String { return \"base\" }\n    func other() -> String { return \"base\" }",
            "public override /* override final */ func speak() -> String",
            "public override /* override final */ func other() -> String",
            transform_swift as Transform,
        ),
    ] {
        let source = format!(
            "class Pet {{\n{base_members}\n}}\n{header} {{\n\
             // override final comment before first member\n\
                 {first} {{ return \"override final\"; }}\n\
                 {second} {{ return \"final override\"; }}\n}}\n"
        );
        let path = Path::new(if language == "swift" {
            "pets.swift"
        } else {
            "pets.ts"
        });
        let classes = parse_classes_in_text(&source, language, path);
        let sub = classes.iter().find(|c| c.name == "Dog").unwrap();
        assert_eq!(sub.members.len(), 2, "selected declarations: {sub:?}");
        let after = transform(&source, sub, "Pet", "pet", &[], Some(&classes[0])).unwrap();
        let classes = parse_classes_in_text(&after, language, path);
        let sub = classes.iter().find(|c| c.name == "Dog").unwrap();
        let expected = if language == "swift" { 4 } else { 3 };
        assert_eq!(
            sub.members.len(),
            expected,
            "methods, delegate and Swift init: {sub:?}"
        );
        for name in ["speak", "other"] {
            let member = sub.members.iter().find(|m| m.name == name).unwrap();
            assert!(member.full_text.contains("public /* override final */"));
            assert!(!member.full_text.contains("public override"));
        }
        assert!(after.contains("// override final comment before first member"));
        assert!(after.contains("return \"override final\""));
        assert!(after.contains("return \"final override\""));
    }
}

#[test]
fn cpp_replacement_preserves_literal_words_in_multiple_members() {
    let source = concat!(
        "#include <string>\n#include <iostream>\n#include <iomanip>\n",
        "class Pet {\npublic:\n    virtual std::string speak(int n = 0) { return \"root\"; }\n",
        "    virtual std::string other() const noexcept(true) { return \"root other\"; }\n};\n",
        "class Dog : public Pet {\npublic:\n",
        "    // override final comment before member\n",
        "    std::string speak(int n = 0) override final { return \" override final\"; }\n",
        "    std::string other() const noexcept(true) override final { return \"final override\"; }\n};\n",
    );
    let case = Case::new("pets.cpp", source, false);
    let before = case.cpp_values(&["Dog"]);
    assert_eq!(before, [" override final"]);
    case.replace_cpp_inheritance();
    let after = case.cpp_values(&["Dog"]);
    case.evidence(&before, &after);
    assert_eq!(after, before);
    case.assert_member(
        "Dog",
        "speak",
        "speak(int n = 0) { return \" override final\"",
    );
    case.assert_member(
        "Dog",
        "other",
        "other() const noexcept(true) { return \"final override\"",
    );
}
