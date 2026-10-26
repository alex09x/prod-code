/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::support::{convert, run};
use prod_code_mcp::to_method::convert_to_method_polyglot;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};

async fn refused(annotations: &[&str]) {
    let mut accepted = Vec::new();
    for annotation in annotations {
        let before = format!(
            "class C {{\n    value = 7;\n    static same(c: C): {annotation} {{ throw Error('stop'); }}\n}}\n"
        );
        let caller = "const result = C.same(new C());\n";
        let ws = Workspace::new(&[("case.ts", &before), ("caller.ts", caller)]);
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
            eprintln!(
                "QUERY_SCOPE_SOURCE {}",
                serde_json::json!({
                    "annotation": annotation, "before": before, "force": force,
                    "after": result.as_ref().ok().map(|r| &r.rewritten[0].1),
                    "error": result.as_ref().err().map(ToString::to_string),
                    "selected_unchanged": std::fs::read_to_string(&path).unwrap() == before,
                    "caller_unchanged": std::fs::read_to_string(ws.root().join("caller.ts")).unwrap() == caller,
                    "line": 3, "column": 5
                })
            );
            if let Ok(value) = result {
                accepted.push(value.rewritten[0].1.clone());
                std::fs::write(&path, &before).unwrap();
                std::fs::write(ws.root().join("caller.ts"), caller).unwrap();
            } else {
                assert!(
                    result
                        .unwrap_err()
                        .to_string()
                        .contains("unsupported TypeScript result type query")
                );
                assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
                assert_eq!(
                    std::fs::read_to_string(ws.root().join("caller.ts")).unwrap(),
                    caller
                );
            }
        }
    }
    assert!(
        accepted.is_empty(),
        "accepted dependent annotations: {accepted:#?}"
    );
}

#[tokio::test]
async fn review24_unicode_query_separators_refuse_atomically() {
    // These are actual TypeScript whitespace characters, not source escapes.
    refused(&[
        "{ value: typeof c.value }",
        "{ value: typeof c.value }",
        "{ value: typeof﻿c.value }",
        "{ value: typeof /* gap */ c.value }",
        "() => typeof c.value",
        "{ value: typeof // query\n c.value }",
    ])
    .await;
}

#[tokio::test]
async fn review24_callable_shadowing_stays_supported() {
    let mut failures = Vec::new();
    for annotation in [
        "(c: { value: string }) => typeof c.value",
        "(c: { value: string }) => typeof c.value",
        "(c: { value: string }) => { value: typeof c.value }",
        "Array<(c: { value: string }) => typeof c.value>",
        "{ read: (c: { value: string }) => typeof c.value }",
        "{ read(c: { value: string }): typeof c.value }",
        "{ (c: { value: string }): typeof c.value }",
        "<T>(c: { value: T }) => typeof c.value",
        r"(\u0063: { value: string }) => typeof c.value",
        "(c: { value: string }, get: () => typeof c.value) => typeof c.value",
        "{ read(c: { value: string }): typeof c.value; other: number }",
        "(c: { value: string }, ...rest: number[]) => typeof c.value",
        "({ value: c }: { value: string }) => typeof c",
        "{ read: (c: { value: string }) => typeof c.value extends string ? typeof c.value : never }",
        "(c: { value: string }) => `value-${typeof c.value}`",
    ] {
        let before = format!(
            "class C {{\n    value = 7;\n    static same(c: C): {annotation} {{ throw Error('stop'); }}\n}}\nconsole.log('ok');\n"
        );
        let result = prod_code_mcp::to_method::polyglot::to_method_ts(&before, Some("C"), "same");
        eprintln!(
            "QUERY_SCOPE_SOURCE {}",
            serde_json::json!({
                "annotation": annotation, "before": before,
                "after": result.as_ref().ok().map(|r| &r.4),
                "error": result.as_ref().err().map(ToString::to_string),
                "line": 3, "column": 5
            })
        );
        if let Err(error) = result {
            failures.push(format!("{annotation}: {error}"));
            continue;
        }
        let (after, _) = convert(&before, "ts", true).await;
        assert!(after.contains(&format!("same(): {annotation}")), "{after}");
        let original_output = run("node", "ts", &before);
        let returned_output = run("node", "ts", &after);
        assert_eq!(returned_output, original_output);
        eprintln!(
            "QUERY_SCOPE_RUNTIME {}",
            serde_json::json!({
                "annotation": annotation, "program": "node", "original_exit": 0,
                "returned_exit": 0, "original_stdout": original_output,
                "returned_stdout": returned_output
            })
        );
    }
    assert!(
        failures.is_empty(),
        "refused independent annotations: {failures:#?}"
    );
}

#[tokio::test]
async fn review24_shadowed_callable_public_result_runs() {
    let before = "class C {\n    value = 7;\n    static same(c: C): (c: { value: string }) => typeof c.value { return item => item.value; }\n}\nconsole.log(C.same(new C())({ value: 'inner' }));\n";
    let (after, _) = convert(before, "ts", true).await;
    eprintln!(
        "QUERY_SCOPE_SOURCE {}",
        serde_json::json!({
            "annotation": "runtime_callback", "before": before, "after": after,
            "line": 3, "column": 5
        })
    );
    let original_output = run("node", "ts", before);
    let returned_output = run("node", "ts", &after);
    assert_eq!(original_output, "inner\n");
    assert_eq!(returned_output, "inner\n");
    eprintln!(
        "QUERY_SCOPE_RUNTIME {}",
        serde_json::json!({
            "annotation": "runtime_callback", "program": "node", "original_exit": 0,
            "returned_exit": 0, "original_stdout": original_output,
            "returned_stdout": returned_output
        })
    );
}

#[tokio::test]
async fn review24_callable_shadowing_does_not_escape_its_scope() {
    refused(&[
        "[(c: { value: string }) => typeof c.value, typeof c.value]",
        "{ read(c: { value: string }): typeof c.value; value: typeof c.value }",
        "{ read(c: { value: string }): typeof c.value\n value: typeof c.value }",
        "(get: (c: { value: string }) => typeof c.value) => typeof c.value",
        "{ label: [c: { value: string }]; value: typeof c.value }",
        "{ read: (c: { value: string }) => typeof c.value; other: () => typeof c.value }",
        "<T extends typeof c.value>(c: { value: string }) => T",
        "({ [c.value]: other }: { [key: number]: string }) => [typeof other, typeof c.value]",
    ])
    .await;
}
