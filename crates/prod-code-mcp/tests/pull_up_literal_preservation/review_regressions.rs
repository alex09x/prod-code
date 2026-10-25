/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::consumer_evidence::record;
use prod_code_mcp::{
    pull_push::{MemberKind, parse_classes_in_text},
    replace_inheritance::{languages::transform_cpp, replace_inheritance_impl},
};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use std::fs;

#[tokio::test]
async fn cpp_opaque_base_refuses_default_forwarding_without_writing() {
    let source = concat!(
        "#include <iostream>\n",
        "class Pet {\npublic:\n",
        "    int speak() noexcept(sizeof(R\"(override final)\") > 0) { return 1; }\n",
        "    int other() { return 7; }\n};\n",
        "class Dog : public Pet {\npublic:\n    Dog() {}\n};\n",
        "int main() { std::cout << Dog().speak() << Dog().other(); }\n",
    );
    // Both omitted and explicitly empty selections request the complete default API.
    for explicit_empty in [false, true] {
        let ws = Workspace::new(&[("pets.cpp", source)]);
        let file = ws.root().join("pets.cpp");
        let classes = parse_classes_in_text(source, "cpp", &file);
        let base = classes.iter().find(|c| c.name == "Pet").unwrap();
        assert_eq!(base.members.len(), 1);
        assert_eq!(base.members[0].kind, MemberKind::Opaque);
        assert!(base.members[0].full_text.contains("int other()"));
        // Empty analyzer diagnostics must not permit dropping the inherited API.
        let gw = ScriptedGateway::start(|method, _| match method {
            "textDocument/diagnostic" => answers::no_diagnostics(),
            _ => serde_json::Value::Null,
        })
        .await;
        let result = replace_inheritance_impl(
            gw.addr(),
            &ws.root(),
            &file,
            "Dog",
            Some("Pet"),
            Some("pet"),
            explicit_empty.then_some(&[]),
            true,
            false,
            None,
        )
        .await;
        match result {
            Ok(result) => {
                eprintln!(
                    "OPAQUE_BASE_BYPASS {}",
                    serde_json::json!({
                        "applied": result.applied,
                        "forwarded_methods": result.forwarded_methods,
                        "diagnostics": result.diagnostics
                    })
                );
                record(
                    "cpp_opaque_base_bypass",
                    "cpp",
                    source,
                    &fs::read_to_string(&file).unwrap(),
                );
                panic!("an opaque base remainder must refuse default forwarding");
            }
            Err(error) => {
                assert!(
                    error
                        .to_string()
                        .contains("cannot safely discover all methods")
                );
                assert_eq!(fs::read_to_string(&file).unwrap(), source);
                record("cpp_opaque_base_refusal", "cpp", source, source);
                eprintln!(
                    "REVIEW_REFUSAL_EVIDENCE {}",
                    serde_json::json!({
                        "label": "cpp_opaque_base", "error": error.to_string(),
                        "explicit_empty": explicit_empty, "unchanged": true
                    })
                );
            }
        }
    }
}

#[tokio::test]
async fn cpp_unsupported_suffix_refuses_instead_of_omitting_the_member() {
    let source = concat!(
        "#include <iostream>\n",
        "class Pet {\npublic:\n    virtual int speak() { return 0; }\n};\n",
        "class Dog : public Pet {\npublic:\n",
        "    int speak() noexcept(sizeof(R\"(x)\") > 0) override { return 1; }\n",
        "    int other() { return 7; }\n};\n",
        "int main() { std::cout << Dog().speak() << Dog().other(); }\n",
    );
    let ws = Workspace::new(&[("pets.cpp", source)]);
    let file = ws.root().join("pets.cpp");
    let classes = parse_classes_in_text(source, "cpp", &file);
    let sub = classes.iter().find(|c| c.name == "Dog").unwrap();
    let base = classes.iter().find(|c| c.name == "Pet").unwrap();
    let result = transform_cpp(source, sub, "Pet", "pet", &[], Some(base));
    match result {
        Ok(after) => {
            record("cpp_unsupported_suffix_bypass", "cpp", source, &after);
            panic!("unsupported suffix should refuse inheritance removal");
        }
        Err(error) => {
            // The refused transformation must preserve the original executable bytes.
            record("cpp_unsupported_suffix_refusal", "cpp", source, source);
            assert!(error.to_string().contains("cannot safely identify C++"));
        }
    }
    let error = replace_inheritance_impl(
        "127.0.0.1:1".parse().unwrap(),
        &ws.root(),
        &file,
        "Dog",
        Some("Pet"),
        Some("pet"),
        None,
        true,
        false,
        None,
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("cannot safely identify C++"));
    assert_eq!(fs::read_to_string(file).unwrap(), source);
    eprintln!(
        "REVIEW_REFUSAL_EVIDENCE {}",
        serde_json::json!({
            "label": "cpp_unsupported_suffix", "error": error.to_string(), "unchanged": true
        })
    );
    let opaque = sub.members.iter().find(|m| m.name == "speak").unwrap();
    assert_eq!(opaque.kind, MemberKind::Opaque);
    assert_eq!(
        &source[opaque.start_offset..opaque.end_offset],
        opaque.full_text
    );
    assert_eq!(sub.members.len(), 1, "opaque tail must not donate members");
}

#[tokio::test]
async fn cpp_opaque_constructor_cannot_hide_a_following_override() {
    let source = concat!(
        "#include <iostream>\n",
        "class Pet {\npublic:\n    virtual int speak() { return 0; }\n};\n",
        "class Dog : public Pet {\npublic:\n",
        "    Dog() noexcept(sizeof(R\"(x)\") > 0) {}\n",
        "    int speak() override { return 1; }\n};\n",
        "int main() { std::cout << Dog().speak(); }\n",
    );
    let ws = Workspace::new(&[("pets.cpp", source)]);
    let file = ws.root().join("pets.cpp");
    let error = replace_inheritance_impl(
        "127.0.0.1:1".parse().unwrap(),
        &ws.root(),
        &file,
        "Dog",
        Some("Pet"),
        Some("pet"),
        None,
        true,
        false,
        None,
    )
    .await
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("cannot safely identify C++ constructor boundary")
    );
    assert_eq!(fs::read_to_string(file).unwrap(), source);
    record("cpp_opaque_constructor_refusal", "cpp", source, source);
    eprintln!(
        "REVIEW_REFUSAL_EVIDENCE {}",
        serde_json::json!({
            "label": "cpp_opaque_constructor", "error": error.to_string(), "unchanged": true
        })
    );
}

#[tokio::test]
async fn cpp_default_discovery_does_not_forward_destructors_or_constructors() {
    for constructor in ["", "    Pet() {}\n"] {
        let source = format!(
            "#include <iostream>\nclass Pet {{\npublic:\n{constructor}\
                 virtual ~Pet() {{}}\n    int speak() {{ return 7; }}\n}};\n\
             class Dog : public Pet {{\npublic:\n    Dog() {{}}\n\
                 ~Dog() override {{ const char* s = \"override final\"; (void)s; }}\n}};\n\
             int main() {{ std::cout << Dog().speak(); }}\n"
        );
        let ws = Workspace::new(&[("pets.cpp", source.as_str())]);
        let file = ws.root().join("pets.cpp");
        // Script only analyzer transport: compiler/runtime controls below establish semantics.
        let gw = ScriptedGateway::start(|method, _| match method {
            "textDocument/diagnostic" => answers::no_diagnostics(),
            _ => serde_json::Value::Null,
        })
        .await;
        let result = replace_inheritance_impl(
            gw.addr(),
            &ws.root(),
            &file,
            "Dog",
            Some("Pet"),
            Some("pet"),
            None,
            true,
            false,
            None,
        )
        .await
        .unwrap();
        let after = fs::read_to_string(file).unwrap();
        record("cpp_default_discovery", "cpp", &source, &after);
        assert_eq!(result.forwarded_methods, ["speak"]);
        assert!(result.applied);
        assert!(!result.verified);
        assert!(after.contains("    Dog() {}"));
        assert!(after.contains("~Dog() { const char* s = \"override final\";"));
        assert!(!after.contains("pet.~Pet"));
    }
}
