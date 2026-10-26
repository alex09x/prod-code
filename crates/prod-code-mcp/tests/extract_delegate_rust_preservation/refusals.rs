/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::support::*;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};

const OWNER: &str = "pub struct Account { pub city: u32, pub balance: u32 }\n";

#[tokio::test]
async fn unsupported_pattern_and_constructor_are_byte_preserving_refusals() {
    for (index, source) in [
        "use crate::account::Account;\nfn f(a: Account) { let Account { city, .. } = a; }\n",
        "use crate::account::Account;\nfn f(a: Account) { let _ = Account { city: 2, ..a }; }\n",
        "use crate::account::Account;\nfn f() { let _ = Account { city: if 1 < 2 { 1 } else { 2 }, balance: 3 }; }\n",
    ].into_iter().enumerate() {
        let ws = Workspace::new(&[("Cargo.toml", MANIFEST), ("src/account.rs", OWNER), ("src/client.rs", source)]);
        let (gw, _) = gateway(&ws, &[("src/client.rs", &["Account { city"])]).await;
        artifact(&format!("refusal-syntax-{index}"), "before", &ws);
        let error = extract(&ws, &gw, &[]).await.unwrap_err().to_string();
        assert!(error.contains("uses `..`") || error.contains("unsupported top-level angle"), "{error}");
        assert_eq!(ws.read("src/account.rs"), OWNER);
        assert_eq!(ws.read("src/client.rs"), source);
        refusal(&format!("refusal-syntax-{index}"), &error, &ws);
    }
}

#[tokio::test]
async fn reference_errors_stale_and_unrecognized_positions_write_nothing() {
    for mode in ["failure", "malformed", "stale", "unrecognized"] {
        let source = "fn f(city: u32) { let _ = city; }\n";
        let ws = Workspace::new(&[
            ("Cargo.toml", MANIFEST),
            ("src/account.rs", OWNER),
            ("src/client.rs", source),
        ]);
        let uri = url::Url::from_file_path(ws.path("src/client.rs"))
            .unwrap()
            .to_string();
        let gw = ScriptedGateway::start(move |method, _| match method {
            "textDocument/references" => match mode {
                "failure" => answers::failure("references unavailable"),
                "malformed" => json!([{"uri": uri}]),
                "stale" => json!([{"uri": uri, "range": {"start": {"line": 0, "character": 0}}}]),
                _ => json!([{"uri": uri, "range": {"start": {"line": 0, "character": 26}}}]),
            },
            "textDocument/diagnostic" => answers::no_diagnostics(),
            _ => Value::Null,
        })
        .await;
        artifact(&format!("refusal-{mode}"), "before", &ws);
        // Force can override diagnostics, never failed/stale/unrecognized references.
        let error = prod_code_mcp::extract_delegate::extract_delegate_rust(
            gw.addr(),
            &ws.root(),
            &ws.path("src/account.rs"),
            Some("Account"),
            None,
            None,
            &["city".into()],
            &[],
            "Address",
            "address",
            true,
            true,
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("cannot find the uses")
                || error.contains("file says otherwise")
                || error.contains("unsupported"),
            "{mode}: {error}"
        );
        assert_eq!(ws.read("src/account.rs"), OWNER);
        assert_eq!(ws.read("src/client.rs"), source);
        refusal(&format!("refusal-{mode}"), &error, &ws);
    }
}

#[tokio::test]
async fn diagnostic_errors_in_external_literal_file_prevent_all_writes() {
    let source =
        "use crate::account::Account;\nfn f() { let _ = Account { city: 1, balance: 2 }; }\n";
    let ws = Workspace::new(&[
        ("Cargo.toml", MANIFEST),
        ("src/account.rs", OWNER),
        ("src/client.rs", source),
    ]);
    let uri = url::Url::from_file_path(ws.path("src/client.rs"))
        .unwrap()
        .to_string();
    let position = source.lines().nth(1).unwrap().find("city").unwrap();
    let gw = ScriptedGateway::start(move |method, params| match method {
        "textDocument/references" => json!([{"uri": uri, "range": {"start": {"line": 1, "character": position}}}]),
        "textDocument/diagnostic" if params["textDocument"]["uri"] == uri => json!({
            "kind": "full", "items": [{"severity": 1, "message": "external fixture error",
                "range": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 1}}}]
        }),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    }).await;
    artifact("refusal-diagnostic", "before", &ws);
    let error = extract(&ws, &gw, &[]).await.unwrap_err().to_string();
    assert!(error.contains("external fixture error"), "{error}");
    assert_eq!(ws.read("src/account.rs"), OWNER);
    assert_eq!(ws.read("src/client.rs"), source);
    refusal("refusal-diagnostic", &error, &ws);
}
