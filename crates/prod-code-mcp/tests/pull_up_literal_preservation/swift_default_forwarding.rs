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
    replace_inheritance::replace_inheritance_impl,
};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use std::fs;

async fn default_forwarding(explicit_initializer: bool) {
    let initializer = if explicit_initializer {
        "    override init() {\n        super.init()\n        let value = \"override final init()\"\n    }\n"
    } else {
        ""
    };
    let source = format!(
        "class Pet {{\n    init() {{}}\n    func speak() -> String {{ return \"base\" }}\n}}\n\
         class Dog: Pet {{\n{initializer}}}\n"
    );
    let mut failures = Vec::new();
    for explicit_empty in [false, true] {
        let ws = Workspace::new(&[("pets.swift", source.as_str())]);
        let file = ws.root().join("pets.swift");
        let classes = parse_classes_in_text(&source, "swift", &file);
        let base = classes.iter().find(|c| c.name == "Pet").unwrap();
        let init = base.members.iter().find(|m| m.name == "init").unwrap();
        // Initializers must remain discoverable for declaration-local modifier stripping.
        assert_eq!(init.kind, MemberKind::Method);
        assert_eq!(&source[init.start_offset..init.end_offset], init.full_text);
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
        let after = fs::read_to_string(&file).unwrap();
        let outcome = match result {
            Ok(result) => {
                assert!(result.applied);
                assert!(result.diagnostics.is_empty());
                assert_eq!(result.forwarded_methods, ["speak"]);
                assert!(!after.contains("class Dog: Pet"));
                assert!(after.contains("return self.pet.speak()"), "{after}");
                let transformed = parse_classes_in_text(&after, "swift", &file);
                let sub = transformed.iter().find(|c| c.name == "Dog").unwrap();
                let inits: Vec<_> = sub.members.iter().filter(|m| m.name == "init").collect();
                assert_eq!(inits.len(), 1, "{after}");
                assert!(inits[0].full_text.contains("self.pet = Pet()"));
                assert!(!inits[0].is_override);
                if explicit_initializer {
                    assert!(
                        inits[0]
                            .full_text
                            .contains("let value = \"override final init()\"")
                    );
                }
                let transformed_base = transformed.iter().find(|c| c.name == "Pet").unwrap();
                assert_eq!(transformed_base.members.len(), base.members.len());
                for (before, after) in base.members.iter().zip(&transformed_base.members) {
                    assert_eq!(before.full_text, after.full_text);
                }
                serde_json::json!({"applied": true, "forwarded_methods": result.forwarded_methods})
            }
            Err(error) => {
                assert_eq!(after, source, "refusal must leave source unchanged");
                failures.push(format!("empty={explicit_empty}: {error}"));
                serde_json::json!({"error": error.to_string(), "unchanged": true})
            }
        };
        let label = format!("swift_default_init_{explicit_initializer}_empty_{explicit_empty}");
        eprintln!(
            "SWIFT_DEFAULT_RESULT {}",
            serde_json::json!({
                "label": label, "explicit_empty": explicit_empty,
                "explicit_initializer": explicit_initializer, "outcome": outcome,
                "limitation": "Source assertions only; scripted analyzer diagnostics are not compiler/runtime proof"
            })
        );
        record(&label, "swift", &source, &after);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn inherited_initializer_is_excluded_from_default_forwarding() {
    default_forwarding(false).await;
}

#[tokio::test]
async fn explicit_initializer_is_preserved_with_default_forwarding() {
    default_forwarding(true).await;
}
