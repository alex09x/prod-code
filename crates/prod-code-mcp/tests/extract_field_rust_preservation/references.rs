/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::evidence;
use super::support::*;
use prod_code_mcp::extract_field::extract;
use prod_code_testkit::{ScriptedGateway, answers};

#[tokio::test]
async fn unread_reference_is_refused_even_with_force() {
    let source = "struct Store {}\nimpl Store {\nfn limit(&self) -> usize { 64 }\n}\nfn main() { assert_eq!(Store {}.limit(), 64); }\n";
    let ws = fixture(source);
    let file = ws.path("src/main.rs");
    let root = ws.root();
    let gateway = ScriptedGateway::start(move |method, request| {
        evidence::reply(
            &root,
            method,
            request,
            match method {
                "textDocument/definition" => answers::locations(&file, &[(1, 8)]),
                // This position spells `fn`, not the declared type. A clean diagnostic is a script.
                "textDocument/references" => answers::locations(&file, &[(3, 1)]),
                "textDocument/diagnostic" => answers::no_diagnostics(),
                _ => serde_json::Value::Null,
            },
        )
    })
    .await;
    let at = source.find("64").unwrap();
    for force in [false, true] {
        evidence::operation(
            &ws.root(),
            &ws.path("src/main.rs"),
            at,
            at + 2,
            serde_json::json!({"all": false, "apply": true, "force": force}),
        );
        let error = extract(
            gateway.addr(),
            &ws.root(),
            &ws.path("src/main.rs"),
            position(source, at),
            position(source, at + 2),
            "cap",
            Some("usize"),
            None,
            false,
            true,
            force,
        )
        .await
        .unwrap_err();
        evidence::emit(serde_json::json!({"kind": "refused", "error": error.to_string()}));
        assert!(error.to_string().contains("were not read"), "{error}");
        assert_eq!(ws.read("src/main.rs"), source);
    }
    assert!(execute(&ws.root(), "unread unchanged").0);
}

#[tokio::test]
async fn rewritten_cross_file_constructor_compiles_and_only_new_field_gains_visibility() {
    let ws = fixture("mod model; mod app;\nfn main() { assert_eq!(app::run(), 64); }\n");
    let model = "pub struct Store { pub entries: usize }\nimpl Store {\npub fn limit(&self) -> usize { 64 }\npub fn new() -> Self { Self { entries: 1 } }\n}\nstruct Private { hidden: usize }\n";
    let app = "use crate::model::Store;\npub fn run() -> usize { Store { entries: 1 }.limit() }\n";
    let model_file = ws.write("src/model.rs", model);
    let app_file = ws.write("src/app.rs", app);
    assert!(execute(&ws.root(), "cross-file before").0);
    let def_file = model_file.clone();
    let refs = [(&model_file, model), (&app_file, app)]
        .iter()
        .flat_map(|(file, text)| {
            text.match_indices("Store")
                .flat_map(|(at, _)| {
                    answers::locations(file, &[position(text, at)])
                        .as_array()
                        .unwrap()
                        .clone()
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let root = ws.root();
    let gateway = ScriptedGateway::start(move |method, request| {
        evidence::reply(
            &root,
            method,
            request,
            match method {
                "textDocument/definition" => answers::locations(&def_file, &[(1, 12)]),
                "textDocument/references" => serde_json::Value::Array(refs.clone()),
                "textDocument/diagnostic" => answers::no_diagnostics(),
                _ => serde_json::Value::Null,
            },
        )
    })
    .await;
    let at = model.find("64").unwrap();
    evidence::operation(
        &ws.root(),
        &model_file,
        at,
        at + 2,
        serde_json::json!({"all": false, "apply": true, "force": false}),
    );
    let result = extract(
        gateway.addr(),
        &ws.root(),
        &model_file,
        position(model, at),
        position(model, at + 2),
        "cap",
        Some("usize"),
        None,
        false,
        true,
        false,
    )
    .await
    .unwrap();
    evidence::result(&ws.root(), &result);
    assert_eq!(result.constructors, 2);
    let changed = ws.read("src/model.rs");
    assert!(changed.contains("pub(crate) cap: usize"));
    assert!(changed.contains("struct Private { hidden: usize }"));
    assert!(execute(&ws.root(), "cross-file applied").0);
}

#[tokio::test]
async fn unrelated_same_spelled_impl_is_not_rewritten_by_self_discovery() {
    let source = "struct Store {}\nimpl Store {\nfn limit(&self) -> usize { 64 }\nfn new() -> Self { Self {} }\n}\nmod other {\nstruct Store {}\nimpl Store { fn new() -> Self { Self {} } }\n}\nfn main() { assert_eq!(Store::new().limit(), 64); }\n";
    let ws = fixture(source);
    let file = ws.path("src/main.rs");
    let refs = [
        source.find("impl Store").unwrap() + 5,
        source.rfind("Store::new").unwrap(),
    ]
    .map(|at| position(source, at));
    let root = ws.root();
    let gateway = ScriptedGateway::start(move |method, request| {
        evidence::reply(
            &root,
            method,
            request,
            match method {
                "textDocument/definition" => answers::locations(&file, &[(1, 8)]),
                "textDocument/references" => answers::locations(&file, &refs),
                "textDocument/diagnostic" => answers::no_diagnostics(),
                _ => serde_json::Value::Null,
            },
        )
    })
    .await;
    let at = source.find("64").unwrap();
    evidence::operation(
        &ws.root(),
        &ws.path("src/main.rs"),
        at,
        at + 2,
        serde_json::json!({"all": false, "apply": true, "force": false}),
    );
    let result = extract(
        gateway.addr(),
        &ws.root(),
        &ws.path("src/main.rs"),
        position(source, at),
        position(source, at + 2),
        "cap",
        Some("usize"),
        None,
        false,
        true,
        false,
    )
    .await
    .unwrap();
    evidence::result(&ws.root(), &result);
    assert_eq!(result.constructors, 1);
    assert!(
        ws.read("src/main.rs")
            .contains("impl Store { fn new() -> Self { Self {} } }")
    );
    assert!(execute(&ws.root(), "owner identity applied").0);
}
