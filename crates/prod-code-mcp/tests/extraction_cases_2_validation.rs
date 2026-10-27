/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::extract_delegate::extract_delegate_polyglot;
use prod_code_mcp::extract_interface::extract_interface_impl;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use std::sync::atomic::{AtomicUsize, Ordering};

#[tokio::test]
async fn rust_interface_dispatch_returns_trait_edits_and_preserves_preview() {
    let source = "pub struct Store;\nimpl Store {\n    pub fn read(&self) -> u32 { 1 }\n}\n";
    let expected = "pub struct Store;\npub trait Reader {\n    fn read(&self) -> u32;\n}\n\nimpl Reader for Store {\n    fn read(&self) -> u32 { 1 }\n}\n";
    let ws = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"sample\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        ("src/lib.rs", source),
    ]);
    let file = ws.path("src/lib.rs");
    let gw = ScriptedGateway::start(|method, _| match method {
        "textDocument/diagnostic" => answers::no_diagnostics(),
        "textDocument/references" => serde_json::json!([]),
        _ => serde_json::Value::Null,
    })
    .await;
    let result = extract_interface_impl(
        gw.addr(),
        &ws.root(),
        &file,
        "Store",
        "Reader",
        &["read".into()],
        2,
        1,
        false,
        false,
        false,
        None,
    )
    .await
    .unwrap();
    assert_eq!(result.type_name, "Store");
    assert_eq!(result.interface_name, "Reader");
    assert_eq!(result.methods, ["read"]);
    assert_eq!(result.overlays, [(file.clone(), expected.into())]);
    assert_eq!(result.files_modified, [file.to_string_lossy().to_string()]);
    assert_eq!(result.diagnostics, Vec::<String>::new());
    assert!(!result.applied);
    // No compiler ran: the analyzer's word is not a verification (#1462).
    assert!(!result.verified);
    assert_eq!(std::fs::read_to_string(file).unwrap(), source);
}

async fn rejecting_gateway() -> ScriptedGateway {
    let diagnostics = AtomicUsize::new(0);
    ScriptedGateway::start(move |method, _| match method {
        "textDocument/diagnostic" => {
            if diagnostics.fetch_add(1, Ordering::SeqCst) == 0 {
                return answers::no_diagnostics();
            }
            let mut report = answers::error_at(2, 3, "E1", "missing type\nextra detail");
            report["items"][0]["source"] = "typed".into();
            let mut second = answers::error_at(3, 1, "E2", "missing name");
            report["items"]
                .as_array_mut()
                .unwrap()
                .push(second["items"][0].take());
            let mut warning = answers::error_at(1, 1, "W1", "unused value");
            warning["items"][0]["severity"] = 2.into();
            report["items"]
                .as_array_mut()
                .unwrap()
                .push(warning["items"][0].take());
            report
        }
        // `Store.value` where the delegate test's source declares it; the delegate needs the
        // field's analyzer identity before it plans anything.
        "textDocument/documentSymbol" => serde_json::json!([{
            "name": "Store", "kind": 5,
            "selectionRange": {"start": {"line": 0, "character": 6}, "end": {"line": 0, "character": 11}},
            "children": [{
                "name": "value", "kind": 8,
                "selectionRange": {"start": {"line": 1, "character": 4}, "end": {"line": 1, "character": 9}}
            }]
        }]),
        _ => serde_json::Value::Null,
    })
    .await
}

#[tokio::test]
async fn interface_validation_rejects_writes_but_preserves_preview_and_force() {
    let source = "class Store {\n    read(): number { return 1; }\n}\n";
    let expected = "export interface Reader {\n    read(): number;\n}\n\nclass Store implements Reader {\n    read(): number { return 1; }\n}\n";
    let diagnostics = vec![
        "[typed] missing type\nextra detail (store.ts:2:3)".to_string(),
        "missing name (store.ts:3:1)".to_string(),
    ];
    for (apply, force) in [(false, false), (true, false), (true, true)] {
        let ws = Workspace::new(&[("store.ts", source)]);
        let file = ws.path("store.ts");
        let gw = rejecting_gateway().await;
        let result = extract_interface_impl(
            gw.addr(),
            &ws.root(),
            &file,
            "Store",
            "Reader",
            &[],
            1,
            1,
            false,
            apply,
            force,
            None,
        )
        .await;
        if apply && !force {
            assert_eq!(
                result.unwrap_err().to_string(),
                format!(
                    "refactoring rejected by validation:\n{}",
                    diagnostics.join("\n")
                )
            );
            assert_eq!(std::fs::read_to_string(&file).unwrap(), source);
        } else {
            let result = result.unwrap();
            assert_eq!(result.diagnostics, diagnostics);
            assert_eq!(result.methods, ["read"]);
            assert_eq!(result.type_name, "Store");
            assert_eq!(result.interface_name, "Reader");
            assert_eq!(result.overlays, [(file.clone(), expected.to_string())]);
            assert_eq!(result.files_modified, [file.to_string_lossy().to_string()]);
            assert_eq!(result.applied, apply);
            assert!(!result.verified);
            assert_eq!(
                std::fs::read_to_string(&file).unwrap(),
                if apply { expected } else { source }
            );
        }
    }
}

#[tokio::test]
async fn interface_dispatch_refusals_do_not_modify_files() {
    for (filename, verify, expected) in [
        // JavaScript is extracted now (#1462); a class with no method has nothing to extract.
        (
            "store.js",
            None,
            "no matching methods found in class `Store` to extract",
        ),
        (
            "store.java",
            None,
            "unsupported language for extract_interface: java",
        ),
    ] {
        let source = "class Store {}";
        let ws = Workspace::new(&[(filename, source)]);
        let file = ws.path(filename);
        let gw = rejecting_gateway().await;
        assert_eq!(
            extract_interface_impl(
                gw.addr(),
                &ws.root(),
                &file,
                "Store",
                "Reader",
                &[],
                1,
                1,
                false,
                true,
                true,
                verify
            )
            .await
            .unwrap_err()
            .to_string(),
            expected
        );
        assert_eq!(std::fs::read_to_string(file).unwrap(), source);
    }
}

#[tokio::test]
async fn delegate_validation_reports_diagnostics_and_obeys_force() {
    let source = "class Store {\n    value: number = 1;\n}\n";
    let expected = "class Details {\n    public value: number;\n\n    constructor(value: number) {\n        this.value = value;\n    }\n}\n\nclass Store {\n\n    public details: Details = new Details(1);\n}\n";
    let diagnostics = vec![
        "missing type [E1] (store.ts:2:3)".to_string(),
        "missing name [E2] (store.ts:3:1)".to_string(),
    ];
    for (apply, force) in [(false, false), (true, false), (true, true)] {
        let ws = Workspace::new(&[("store.ts", source)]);
        let file = ws.path("store.ts");
        let gw = rejecting_gateway().await;
        let result = extract_delegate_polyglot(
            gw.addr(),
            &ws.root(),
            &file,
            Some("Store"),
            None,
            None,
            &["value".into()],
            &[],
            "Details",
            "details",
            apply,
            force,
            None,
        )
        .await;
        if apply && !force {
            assert_eq!(
                result.unwrap_err().to_string(),
                format!(
                    "the change does not compile (2 error(s)); nothing was written:\n  {}",
                    diagnostics.join("\n  ")
                )
            );
            assert_eq!(std::fs::read_to_string(file).unwrap(), source);
        } else {
            let result = result.unwrap();
            assert_eq!(result.diagnostics, diagnostics);
            assert_eq!(result.unmatched, Vec::<String>::new());
            assert_eq!(
                result.rewritten,
                [(file.to_string_lossy().to_string(), expected.to_string())]
            );
            assert_eq!(result.applied, apply);
            assert_eq!(result.accesses, 0);
            assert_eq!(
                std::fs::read_to_string(file).unwrap(),
                if apply { expected } else { source }
            );
        }
    }
}
