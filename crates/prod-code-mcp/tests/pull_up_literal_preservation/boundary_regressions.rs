/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{consumer_evidence::record, shared_consumers::replace, support::Case};
use prod_code_mcp::pull_push::parse_classes_in_text;
use std::path::Path;

fn assert_separate_members(source: &str, language: &str, names: &[&str]) {
    let classes = parse_classes_in_text(source, language, Path::new("pets"));
    let dog = classes.iter().find(|class| class.name == "Dog").unwrap();
    assert_eq!(
        dog.members
            .iter()
            .map(|member| member.name.as_str())
            .collect::<Vec<_>>(),
        names
    );
    for pair in dog.members.windows(2) {
        assert!(pair[0].end_offset <= pair[1].start_offset, "{pair:?}");
    }
    for member in &dog.members {
        assert_eq!(
            &source[member.start_offset..member.end_offset],
            member.full_text
        );
    }
}

#[test]
fn typescript_template_parameters_survive_replacement() {
    for default in ["`(`", "`override final ; { (`", "`(${(() => 7)()}`"] {
        let source = format!(
            "class Pet {{\n    speak(value = {default}) {{ return value; }}\n}}\n\
             class Dog extends Pet {{\n    constructor() {{ super(); }}\n\
                 override speak(value = {default}) {{ return value; }}\n\
                 other() {{ return \"override final\"; }}\n}}\n\
             console.log(JSON.stringify([new Dog().speak(), new Dog().other()]));\n"
        );
        let after = replace(&source, "typescript");
        record(
            "typescript_template_parameters",
            "typescript",
            &source,
            &after,
        );
        assert_separate_members(&source, "typescript", &["speak", "other"]);
        assert!(after.contains(&format!("    speak(value = {default})")));
        assert!(!after.contains("override speak"));
    }
}

#[test]
fn cpp_suffix_delimiters_survive_replacement() {
    for suffix in [
        "/* ; */ override",
        "/* { ; */ noexcept(sizeof(\";{override final\") > 0) override final",
        "noexcept(noexcept(std::string{\";{override final\"})) override",
    ] {
        let source = format!(
            "#include <iostream>\n#include <string>\n\
             class Pet {{\npublic:\n    virtual ~Pet() {{}}\n\
                 virtual std::string speak() {{ return \"base\"; }}\n}};\n\
             class Dog : public Pet {{\npublic:\n    Dog() {{}}\n\
                 ~Dog() /* ; */ override {{}}\n\
                 std::string speak() {suffix} {{ return \"override final\"; }}\n\
                 int other() {{ return 7; }}\n}};\n\
             int main() {{ std::cout << Dog().speak() << Dog().other(); }}\n"
        );
        let after = replace(&source, "cpp");
        record("cpp_suffix_delimiters", "cpp", &source, &after);
        assert_separate_members(&source, "cpp", &["Dog", "~Dog", "speak", "other"]);
        assert!(after.contains("    Dog() {}"));
        assert!(after.contains("~Dog() /* ; */ {}"));
        assert!(after.contains("return \"override final\";"));
        assert!(!after.contains(" override"));
    }
}

#[test]
fn typescript_decorated_fields_have_disjoint_ranges() {
    let source = concat!(
        "function memo(_target: any, _key: string) {}\n",
        "function tag(_value: any) { return memo; }\n",
        "class Pet {\n    value: string = \"base\";\n    otherValue: string = \"base\";\n}\n",
        "class Dog extends Pet {\n    constructor() { super(); }\n",
        "    @memo\n    override value: string = \"override final\";\n",
        "    @tag({ override: `override ( final`, nested: [(\"override\")] })\n",
        "    override otherValue: string = \"other\";\n",
        "    other() { return 1; }\n}\n",
        "console.log(JSON.stringify([new Dog().value, new Dog().otherValue, new Dog().other()]));\n",
    );
    let after = replace(source, "typescript");
    record("typescript_decorated_fields", "typescript", source, &after);
    assert_separate_members(source, "typescript", &["value", "otherValue", "other"]);
    assert_separate_members(
        &after,
        "typescript",
        &["value", "otherValue", "other", "pet"],
    );
    assert!(after.contains("@memo\n    value: string = \"override final\";"));
    assert!(after.contains(
        "@tag({ override: `override ( final`, nested: [(\"override\")] })\n    otherValue"
    ));
    assert!(after.contains("other() { return 1; }"));
}

#[tokio::test]
async fn typescript_template_parameter_pull_preserves_literal_siblings() {
    let source = concat!(
        "interface Pet { speak(value?: string): string; }\nclass Pet {}\n",
        "class Dog extends Pet {\n    override speak(value = `(`) { return value; }\n}\n",
        "class Copy extends Pet {\n    override speak(value = `(`) { return value; }\n}\n",
        "class Cat extends Pet {\n    override speak(value = `)`) { return value; }\n}\n",
        "console.log(JSON.stringify([new Dog().speak(), new Copy().speak(), new Cat().speak()]));\n",
    );
    let case = Case::new("pets.ts", source, false);
    case.pull().await;
    let after = case.source();
    record(
        "typescript_template_parameter_pull",
        "typescript",
        source,
        &after,
    );
    case.assert_member("Pet", "speak", "speak(value = `(`) { return value; }");
    case.assert_no_member("Dog", "speak");
    case.assert_no_member("Copy", "speak");
    case.assert_member(
        "Cat",
        "speak",
        "override speak(value = `)`) { return value; }",
    );
}
