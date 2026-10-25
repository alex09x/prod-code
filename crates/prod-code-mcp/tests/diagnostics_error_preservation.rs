/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::diagnostics::{validate_text, validate_texts};
use prod_code_mcp::tools::execute_tool;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Mutex;

#[path = "diagnostics_error_preservation/aliases.rs"]
mod aliases;
#[path = "diagnostics_error_preservation/compiler.rs"]
mod compiler;
#[path = "diagnostics_error_preservation/const_headers.rs"]
mod const_headers;
#[path = "diagnostics_error_preservation/impl_macro_headers.rs"]
mod impl_macro_headers;
#[path = "diagnostics_error_preservation/macro_headers.rs"]
mod macro_headers;
#[path = "diagnostics_error_preservation/ranges.rs"]
mod ranges;

const BEFORE: &str = "fn first() {\n    let value = false;\n    consume(value);\n}\nfn second() {\n    let value = 0;\n    consume(value);\n}\n";
const AFTER: &str = "fn first() {\n    let value = 0;\n    consume(value);\n}\nfn second() {\n    let value = false;\n    consume(value);\n}\n";

/// Model the analyzer's answers to the actual baseline/proposal overlays, not query order.
async fn gateway(
    ws: &Workspace,
    diagnose: impl Fn(&str) -> Value + Send + Sync + 'static,
) -> ScriptedGateway {
    gateway_for(ws, "src/lib.rs", diagnose).await
}

async fn gateway_for(
    ws: &Workspace,
    path: &str,
    diagnose: impl Fn(&str) -> Value + Send + Sync + 'static,
) -> ScriptedGateway {
    let disk = ws.read(path);
    let opened = Mutex::new(HashMap::<String, String>::new());
    ScriptedGateway::start(move |method, params| {
        let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
        let mut opened = opened.lock().unwrap();
        match method {
            "prod-code/handshake" => opened.clear(),
            "textDocument/didOpen" => {
                opened.insert(
                    uri.into(),
                    params["textDocument"]["text"].as_str().unwrap().into(),
                );
            }
            "textDocument/didChange" => {
                opened.insert(
                    uri.into(),
                    params["contentChanges"][0]["text"].as_str().unwrap().into(),
                );
            }
            "textDocument/diagnostic" => {
                return diagnose(opened.get(uri).unwrap_or(&disk));
            }
            _ => {}
        }
        json!([])
    })
    .await
}

fn workspace(text: &str) -> Workspace {
    Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"error-preservation\"\nversion = \"0.0.0\"\nedition = \"2021\"\n",
        ),
        ("src/lib.rs", text),
    ])
}

fn occurrence(text: &str) -> Value {
    let line = text
        .lines()
        .position(|line| line.contains("false"))
        .unwrap() as u32
        + 2;
    answers::error_at(line, 5, "E0308", "expected u32, found bool")
}

#[tokio::test]
async fn replacing_an_error_with_an_identical_error_in_another_occurrence_is_blocking() {
    let ws = workspace(BEFORE);
    let gw = gateway(&ws, occurrence).await;
    let report = validate_text(gw.addr(), &ws.root(), &ws.path("src/lib.rs"), AFTER)
        .await
        .unwrap();
    assert!(
        !report.ok(),
        "new occurrence was certified clean: {report:?}"
    );
    assert_eq!(report.errors, 1);
    assert_eq!(report.items[0].line, 7);
    assert!(report.preexisting.is_empty());

    let reports = validate_texts(
        gw.addr(),
        &ws.root(),
        &[(ws.path("src/lib.rs"), AFTER.into())],
        &[],
    )
    .await
    .unwrap();
    assert!(!reports[0].ok(), "batch lost the new occurrence");
    assert_eq!(ws.read("src/lib.rs"), BEFORE);
}

#[tokio::test]
async fn public_validation_result_refuses_each_unproven_error() {
    for (before, after, code, message) in [
        (BEFORE, AFTER, "E0308", "expected u32, found bool"),
        (
            "",
            "#[derive(Broken)]\nstruct S;\n",
            "E0282",
            "type annotations needed",
        ),
        (
            "",
            "fn run() {}\n",
            "E0277",
            "`Rc<u8>` cannot be sent between threads safely",
        ),
    ] {
        let ws = workspace(before);
        let gw = gateway(&ws, move |text| {
            if code == "E0308" {
                occurrence(text)
            } else if text.is_empty() {
                answers::no_diagnostics()
            } else {
                answers::error_at(1, 1, code, message)
            }
        })
        .await;
        let result = execute_tool(
            gw.addr(),
            &ws.root(),
            "code_validate_edits",
            json!({
                "edits": [{"path": "src/lib.rs", "new_text": after}]
            }),
        )
        .await
        .unwrap();
        assert!(
            result.is_error,
            "{code} became a successful public tool result: {result:?}"
        );
        let rendered = serde_json::to_string(&result).unwrap();
        assert!(rendered.contains(code), "{rendered}");
        assert!(rendered.contains("1 error(s)"), "{rendered}");
        assert_eq!(ws.read("src/lib.rs"), before);
    }
}

#[tokio::test]
async fn unchanged_occurrences_shift_but_ambiguous_correspondence_stays_blocking() {
    let ws = workspace(BEFORE);
    let gw = gateway(&ws, occurrence).await;
    let shifted = format!("// harmless insertion\n{BEFORE}");
    let report = validate_text(gw.addr(), &ws.root(), &ws.path("src/lib.rs"), &shifted)
        .await
        .unwrap();
    assert!(report.ok(), "unchanged shifted occurrence: {report:?}");
    assert_eq!(report.preexisting.len(), 1);
    assert_eq!(report.preexisting[0].line, 4);

    let ws = workspace("missing();\nmissing();\n");
    let gw = gateway(&ws, |_| {
        answers::error_at(1, 1, "E0425", "cannot find function `missing`")
    })
    .await;
    let report = validate_text(
        gw.addr(),
        &ws.root(),
        &ws.path("src/lib.rs"),
        "missing();\n",
    )
    .await
    .unwrap();
    assert!(
        !report.ok(),
        "either identical old occurrence could have survived: {report:?}"
    );
}

#[tokio::test]
async fn analyzer_failures_cannot_be_consumed_as_baseline() {
    for code in [
        prod_code_protocol::ANALYZER_PANIC_CODE,
        "unlinked-file",
        "prod-code-invalid-diagnostics",
    ] {
        let ws = workspace("#[cfg(test)]\nfn run() {}\n");
        let gw = gateway(&ws, move |_| {
            answers::error_at(1, 1, code, "not validated: unresolved macro assert_eq!")
        })
        .await;
        let report = validate_text(
            gw.addr(),
            &ws.root(),
            &ws.path("src/lib.rs"),
            "// shift\n#[cfg(test)]\nfn run() {}\n",
        )
        .await
        .unwrap();
        assert!(!report.ok(), "{code}: {report:?}");
        assert!(report.preexisting.is_empty());
    }
}
