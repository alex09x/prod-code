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

#[tokio::test]
async fn cpp_declaration_modifiers_are_removed_without_touching_literal_words() {
    let source = concat!(
        "#include <string>\n#include <iostream>\n#include <iomanip>\n",
        "class Root {\npublic:\n    virtual std::string speak() { return \"root\"; }\n};\n",
        "class Pet : public Root {\npublic:\n};\n",
        "class Dog : public Pet {\npublic:\n",
        "    std::string speak() override final { return \" override final\"; }\n};\n",
        "class Cat : public Pet {\npublic:\n",
        "    std::string speak() override final { return \" final\"; }\n};\n",
    );
    let case = Case::new("pets.cpp", source, false);
    case.assert_member(
        "Dog",
        "speak",
        "override final { return \" override final\"",
    );
    let before = case.cpp_values(&["Dog", "Cat"]);
    assert_eq!(before, [" override final", " final"]);
    case.pull().await;
    let after = case.cpp_values(&["Dog", "Cat"]);
    case.evidence(&before, &after);
    assert_eq!(after, before);
    case.assert_member("Pet", "speak", "speak() { return \" override final\"");
    case.assert_member("Cat", "speak", "return \" final\"");
    case.assert_no_member("Dog", "speak");
}

#[tokio::test]
async fn cpp_default_arguments_do_not_relocate_final_onto_the_parent() {
    for parameter in [
        "int n = 0",
        "int n = (0)",
        "const char* n = \"override final\"",
        "char n = ')'",
    ] {
        let source = format!(
            "#include <string>\n#include <iostream>\n#include <iomanip>\n\
             class Root {{\npublic:\n    virtual std::string speak({parameter}) {{ return \"root\"; }}\n}};\n\
             class Pet : public Root {{\npublic:\n}};\n\
             class Dog : public Pet {{\npublic:\n    std::string speak({parameter}) override final {{ return \" override final\"; }}\n}};\n\
             class Cat : public Pet {{\npublic:\n    std::string speak({parameter}) override final {{ return \" final\"; }}\n}};\n"
        );
        let case = Case::new("pets.cpp", &source, false);
        case.assert_member(
            "Dog",
            "speak",
            &format!("speak({parameter}) override final"),
        );
        let before = case.cpp_values(&["Dog", "Cat"]);
        assert_eq!(before, [" override final", " final"]);
        case.pull().await;
        let after = case.cpp_values(&["Dog", "Cat"]);
        case.evidence(&before, &after);
        assert_eq!(after, before);
        case.assert_member("Pet", "speak", &format!("speak({parameter}) {{"));
        case.assert_member("Cat", "speak", "override final { return \" final\"");
        case.assert_no_member("Dog", "speak");
    }
}

#[tokio::test]
async fn unsupported_cpp_declarator_refuses_before_writing() {
    let source = concat!(
        "#include <string>\n#include <iostream>\n#include <iomanip>\n",
        "class Root {\npublic:\n    virtual auto speak() -> std::string { return \"root\"; }\n};\n",
        "class Pet : public Root {\npublic:\n};\n",
        "class Dog : public Pet {\npublic:\n",
        "    auto speak() -> std::string override final { return \" override final\"; }\n};\n",
        "class Cat : public Pet {\npublic:\n",
        "    auto speak() -> std::string override final { return \" final\"; }\n};\n",
    );
    let case = Case::new("pets.cpp", source, false);
    let before = case.cpp_values(&["Dog", "Cat"]);
    assert_eq!(before, [" override final", " final"]);
    case.assert_pull_refused("cannot safely identify C++ declaration modifiers")
        .await;
    let after = case.cpp_values(&["Dog", "Cat"]);
    assert_eq!(after, before);
}
