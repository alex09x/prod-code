/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{consumer_evidence::record, support::Case};
use prod_code_mcp::{
    pull_push::{parse_classes_in_text, strip_override_modifiers},
    replace_inheritance::languages::{transform_cpp, transform_swift, transform_typescript},
};
use std::path::Path;

pub(super) fn replace(source: &str, language: &str) -> String {
    let path = format!("pets.{language}");
    let classes = parse_classes_in_text(source, language, Path::new(&path));
    let sub = classes.iter().find(|c| c.name == "Dog").unwrap();
    let base = classes.iter().find(|c| c.name == "Pet").unwrap();
    for member in &sub.members {
        assert_eq!(
            &source[member.start_offset..member.end_offset],
            member.full_text
        );
    }
    match language {
        "cpp" => transform_cpp(source, sub, "Pet", "pet", &[], Some(base)),
        "swift" => transform_swift(source, sub, "Pet", "pet", &[], Some(base)),
        _ => transform_typescript(source, sub, "Pet", "pet", &[], Some(base)),
    }
    .unwrap()
}

#[test]
fn cpp_destructor_replacement_keeps_constructors_and_literal_bytes() {
    let source = concat!(
        "#include <iostream>\n#include <string>\n",
        "class Pet {\npublic:\n    virtual ~Pet() {}\n};\n",
        "class Dog : public Pet {\npublic:\n",
        "    Dog() {}\n",
        "    ~Dog() override final { const char* s = \"override final\"; (void)s; }\n",
        "    std::string speak() { return \"override final\"; }\n};\n",
        "int main() { std::cout << Dog().speak(); }\n",
    );
    let after = replace(source, "cpp");
    record("cpp_destructor", "cpp", source, &after);
    assert!(after.contains("    Dog() {}"));
    assert!(after.contains("~Dog() { const char* s = \"override final\""));
    let classes = parse_classes_in_text(&after, "cpp", Path::new("pets.cpp"));
    let sub = classes.iter().find(|c| c.name == "Dog").unwrap();
    assert!(sub.members.iter().any(|m| m.name == "~Dog"));
}

#[test]
fn cpp_declared_and_defaulted_destructors_keep_the_next_member_separate() {
    for (destructor, definition) in [
        ("~Dog() override final;", "Dog::~Dog() {}\n"),
        ("~Dog() override final = default;", ""),
    ] {
        let source = format!(
            "#include <iostream>\n#include <string>\n\
             class Pet {{\npublic:\n    virtual ~Pet() {{}}\n}};\n\
             class Dog : public Pet {{\npublic:\n    Dog() {{}}\n    {destructor}\n\
                 std::string speak() {{ return \"override final\"; }}\n}};\n\
             {definition}int main() {{ std::cout << Dog().speak(); }}\n"
        );
        let after = replace(&source, "cpp");
        record("cpp_destructor_declaration", "cpp", &source, &after);
        let classes = parse_classes_in_text(&after, "cpp", Path::new("pets.cpp"));
        let sub = classes.iter().find(|c| c.name == "Dog").unwrap();
        let destructor = sub.members.iter().find(|m| m.name == "~Dog").unwrap();
        assert!(!destructor.full_text.contains("speak"));
        assert!(!destructor.full_text.contains("override"));
        assert!(sub.members.iter().any(|m| m.name == "speak"));
        assert!(after.contains("    Dog() {}"));
    }
}

#[test]
fn typescript_same_name_method_replacement() {
    let source = concat!(
        "class Pet {\n    Dog() { return \"base\"; }\n}\n",
        "class Dog extends Pet {\n    constructor() { super(); }\n",
        "    override Dog() { return \"override final\"; }\n}\n",
        "console.log(new Dog().Dog());\n",
    );
    let after = replace(source, "typescript");
    record("typescript_same_name", "typescript", source, &after);
    assert!(after.contains("    Dog() { return \"override final\"; }"));
}

#[test]
fn swift_same_name_method_replacement() {
    let source = concat!(
        "class Pet {\n    func Dog() -> String { return \"base\" }\n}\n",
        "class Dog: Pet {\n    override func Dog() -> String { return \"override final\" }\n}\n",
    );
    let after = replace(source, "swift");
    record("swift_same_name", "swift", source, &after);
    assert!(
        after.contains("    func Dog() -> String { return \"override final\" }"),
        "{after}"
    );
    assert!(!after.contains("override func Dog"), "{after}");
}

const DECORATOR_SETUP: &str = concat!(
    "function memo(_target: any, _key: string, _descriptor: PropertyDescriptor) {}\n",
    "function tag(_value: any) { return memo; }\n",
);

#[test]
fn typescript_decorator_replacement_preserves_arguments_and_boundaries() {
    for decorator in [
        "@memo",
        "@tag({ override: \"override ) final\", nested: [(\"override\")] }) @memo",
        "@tag(\n        { override: \"override ) final\", nested: [(\"override\")] }\n    )\n    @memo",
    ] {
        let source = format!(
            "{DECORATOR_SETUP}class Pet {{\n    speak() {{ return \"base\"; }}\n}}\n\
             class Dog extends Pet {{\n    constructor() {{ super(); }}\n\
                 {decorator} override speak() {{ return `override final \n        tail`; }}\n\
                 other() {{ return \"override final\"; }}\n}}\n\
             console.log(JSON.stringify([new Dog().speak(), new Dog().other()]));\n"
        );
        let after = replace(&source, "typescript");
        record(
            "typescript_decorator_replacement",
            "typescript",
            &source,
            &after,
        );
        assert!(after.contains(&format!("{decorator} speak()")), "{after}");
        assert!(after.contains("`override final \n        tail`"));
        let classes = parse_classes_in_text(&after, "typescript", Path::new("pets.ts"));
        let sub = classes.iter().find(|c| c.name == "Dog").unwrap();
        let speak = sub.members.iter().find(|m| m.name == "speak").unwrap();
        assert!(speak.full_text.contains(decorator));
        assert!(!speak.full_text.contains("other()"));
    }
}

#[tokio::test]
async fn typescript_decorator_pull_to_root_preserves_moved_text_and_siblings() {
    for decorator in [
        "@memo",
        "@tag({ override: \"override ) final\", nested: [(\"override\")] })\n    @memo",
    ] {
        // Declaration merging makes the original override valid even for an empty root.
        let source = format!(
            "{DECORATOR_SETUP}interface Pet {{ speak(): string; }}\nclass Pet {{}}\n\
         class Dog extends Pet {{\n    {decorator} override speak() {{ return \"override final\"; }}\n}}\n\
         class Copy extends Pet {{\n    {decorator} override speak() {{ return \"override final\"; }}\n}}\n\
         class Cat extends Pet {{\n    {decorator} override speak() {{ return \"final\"; }}\n}}\n\
         console.log(JSON.stringify([new Dog().speak(), new Copy().speak(), new Cat().speak()]));\n"
        );
        let case = Case::new("pets.ts", &source, false);
        case.pull().await;
        let after = case.source();
        record("typescript_decorator_pull", "typescript", &source, &after);
        case.assert_member("Pet", "speak", "@memo speak() { return \"override final\";");
        case.assert_no_member("Dog", "speak");
        case.assert_no_member("Copy", "speak");
        case.assert_member("Cat", "speak", "@memo override speak() { return \"final\";");
        assert!(after.contains(&format!("{decorator} speak()")));
    }
}

#[test]
fn decorator_arguments_cannot_donate_modifier_tokens() {
    for decorator in [
        "@tag({ override: \"override )\", nested: [(\"final\")] })",
        "@tag(\n    { override: \"override )\", nested: [(\"final\")] }\n) @memo",
    ] {
        let declaration =
            format!("{decorator} public override speak() {{ return \"override final\"; }}");
        assert_eq!(
            strip_override_modifiers(&declaration, "typescript"),
            format!("{decorator} public speak() {{ return \"override final\"; }}")
        );
        let ordinary = format!("{decorator} override() {{ return \"override final\"; }}");
        assert_eq!(strip_override_modifiers(&ordinary, "typescript"), ordinary);
        let source = format!("class Dog {{\n    {decorator} speak() {{}}\n}}\n");
        let classes = parse_classes_in_text(&source, "typescript", Path::new("pets.ts"));
        let member = &classes[0].members[0];
        assert_eq!(member.name, "speak");
        assert!(
            !member.is_override,
            "decorator argument donated override: {member:?}"
        );
    }
}
