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
use prod_code_mcp::pull_push::{parse_classes_in_text, push_down_impl};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use std::{
    fs,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

#[tokio::test]
async fn cpp_opaque_push_down_refuses_before_relocating_following_members() {
    let source = concat!(
        "#include <iostream>\n",
        "class Pet {\npublic:\n",
        "    int speak() noexcept(sizeof(R\"(x)\") > 0) { return 1; }\n",
        "    int other() { return 7; }\n};\n",
        "class Dog : public Pet {\n};\n",
        "int main() { std::cout << Dog().speak() << Dog().other(); }\n",
    );
    let ws = Workspace::new(&[("pets.cpp", source)]);
    let file = ws.root().join("pets.cpp");
    let requests = Arc::new(AtomicUsize::new(0));
    let seen = requests.clone();
    let gw = ScriptedGateway::start(move |method, _| {
        seen.fetch_add(1, Ordering::SeqCst);
        match method {
            "textDocument/diagnostic" => answers::no_diagnostics(),
            _ => serde_json::Value::Null,
        }
    })
    .await;
    for apply in [true, false] {
        for force in [false, true] {
            let result = push_down_impl(
                gw.addr(),
                &ws.root(),
                &file,
                "Pet",
                Some(&["Dog".into()]),
                &["speak".into()],
                apply,
                force,
                None,
            )
            .await;
            let after = fs::read_to_string(&file).unwrap();
            if apply && !force {
                // Both buggy and refused programs compile/run identically: a runtime
                // smoke check alone cannot detect the loss of Pet::other's API.
                record("cpp_opaque_push_down", "cpp", source, &after);
                let classes = parse_classes_in_text(&after, "cpp", &file);
                eprintln!(
                    "PUSH_DOWN_EVIDENCE {}",
                    serde_json::json!({
                        "label": "cpp_opaque_push_down", "apply": apply, "force": force,
                        "source_unchanged": after == source,
                        "pet_body": &after[classes[0].body_start..classes[0].body_end],
                        "result": result.as_ref().map(|_| "accepted").map_err(|e| e.to_string()),
                        "analyzer_requests": requests.load(Ordering::SeqCst)
                    })
                );
            }
            let error = result.expect_err("opaque tail must not be relocated as one method");
            assert!(
                error
                    .to_string()
                    .contains("cannot safely identify C++ member boundary")
            );
            assert_eq!(
                after, source,
                "refusal must preserve speak and the following other"
            );
            assert_eq!(
                requests.load(Ordering::SeqCst),
                0,
                "refuse before analyzer access"
            );
        }
    }
}

#[tokio::test]
async fn cpp_ordinary_push_down_keeps_other_api_access_and_literal_bytes() {
    let source = concat!(
        "#include <iostream>\n",
        "class Pet {\npublic:\n",
        "    int speak() noexcept(sizeof(\"override final\") > 0) { return 1; }\n",
        "    int other() { return 7; }\n};\n",
        "class Dog : public Pet {\n};\n",
        "int main() { std::cout << Pet().other() << Dog().speak() << Dog().other(); }\n",
    );
    let ws = Workspace::new(&[("pets.cpp", source)]);
    let file = ws.root().join("pets.cpp");
    let gw = ScriptedGateway::start(|method, _| match method {
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    })
    .await;
    let result = push_down_impl(
        gw.addr(),
        &ws.root(),
        &file,
        "Pet",
        Some(&["Dog".into()]),
        &["speak".into()],
        true,
        false,
        None,
    )
    .await
    .unwrap();
    assert!(result.applied);
    assert!(
        !result.verified,
        "scripted diagnostics are not compiler evidence"
    );
    let after = fs::read_to_string(&file).unwrap();
    let classes = parse_classes_in_text(&after, "cpp", &file);
    let pet = classes.iter().find(|c| c.name == "Pet").unwrap();
    let dog = classes.iter().find(|c| c.name == "Dog").unwrap();
    assert_eq!(
        pet.members
            .iter()
            .map(|m| m.name.as_str())
            .collect::<Vec<_>>(),
        ["other"]
    );
    assert_eq!(
        dog.members
            .iter()
            .map(|m| m.name.as_str())
            .collect::<Vec<_>>(),
        ["speak"]
    );
    assert_eq!(
        dog.members[0].full_text.trim_start_matches([' ', '\t']),
        "int speak() noexcept(sizeof(\"override final\") > 0) { return 1; }"
    );
    record("cpp_ordinary_push_down", "cpp", source, &after);
}
