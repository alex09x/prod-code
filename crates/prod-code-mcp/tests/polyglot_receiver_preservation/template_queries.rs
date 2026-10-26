/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::to_method::convert_to_method_polyglot;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};

#[tokio::test]
async fn review26_template_query_boundaries() {
    // These exact sources are compiler-checked before public conversion. JSON
    // escaping represents one source backslash in the escaped-dollar literals.
    let cases: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "../../../../evidence/codex-review-audit/issue-1317/repair26/cases.json"
    ))
    .unwrap();
    let mut failures = Vec::new();
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let annotation = case["annotation"].as_str().unwrap();
        let before = case["before"].as_str().unwrap();
        let refuse = case["refuse"].as_bool().unwrap();
        let caller = "export const untouched = 'caller';\n";
        let ws = Workspace::new(&[("case.ts", before), ("caller.ts", caller)]);
        let path = ws.root().join("case.ts");
        let gateway = ScriptedGateway::start(|method, _| match method {
            "textDocument/diagnostic" => answers::no_diagnostics(),
            _ => serde_json::Value::Null,
        })
        .await;
        for force in [false, true] {
            let result = convert_to_method_polyglot(
                gateway.addr(),
                &ws.root(),
                &path,
                Some("C"),
                "same",
                true,
                force,
            )
            .await;
            let after = result.as_ref().ok().map(|r| r.rewritten[0].1.clone());
            let error = result.as_ref().err().map(ToString::to_string);
            let selected_unchanged = std::fs::read_to_string(&path).unwrap() == before;
            let caller_unchanged =
                std::fs::read_to_string(ws.root().join("caller.ts")).unwrap() == caller;
            eprintln!(
                "TEMPLATE_QUERY_SOURCE {}",
                serde_json::json!({
                    "name": name, "annotation": annotation, "before": before, "after": after,
                    "error": error, "force": force, "refuse": refuse,
                    "selected_unchanged": selected_unchanged, "caller_unchanged": caller_unchanged,
                    "line": 4, "column": 5
                })
            );
            if refuse {
                if !error
                    .as_ref()
                    .is_some_and(|e| e.contains("unsupported TypeScript result type query"))
                    || !selected_unchanged
                    || !caller_unchanged
                {
                    failures.push(format!("{name} force={force}: expected atomic refusal"));
                }
            } else if let Some(after) = after {
                assert!(after.contains(&format!("same(): {annotation}")), "{after}");
                assert_eq!(std::fs::read_to_string(&path).unwrap(), after);
                assert!(caller_unchanged);
            } else {
                failures.push(format!("{name} force={force}: {error:?}"));
            }
            std::fs::write(&path, before).unwrap();
            std::fs::write(ws.root().join("caller.ts"), caller).unwrap();
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
