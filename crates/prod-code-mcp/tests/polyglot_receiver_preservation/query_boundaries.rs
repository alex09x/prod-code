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
async fn review25_query_identifier_and_conditional_boundaries() {
    let mut failures = Vec::new();
    for (name, prelude, annotation, refuse) in [
        (
            "conditional_branch",
            "type c = string;",
            "{ value: number extends string ? (c) : typeof c.value }",
            true,
        ),
        (
            "nested_conditional_branch",
            "type c = string;",
            "{ value: number extends string ? string extends number ? never : (c) : typeof c.value }",
            true,
        ),
        (
            "arrow_conditional_branch",
            "type c = string;",
            "{ value: number extends string ? () => (c) : typeof c.value }",
            true,
        ),
        (
            "combining_mark",
            "const c\u{301} = { value: 1 };",
            "{ value: typeof c\u{301}.value }",
            false,
        ),
        (
            "escaped_combining_mark",
            "const c\u{301} = { value: 1 };",
            r"{ value: typeof c\u0301.value }",
            false,
        ),
        (
            "joiner",
            "const c\u{200d} = { value: 1 };",
            "{ value: typeof c\u{200d}.value }",
            false,
        ),
        ("untyped_method", "", "{ read(c): typeof c.value }", false),
        (
            "untyped_optional_method",
            "",
            "{ read?(c): typeof c.value }",
            false,
        ),
        (
            "untyped_call_signature",
            "",
            "{ (c): typeof c.value }",
            false,
        ),
        (
            "untyped_generic_method",
            "",
            "{ read<T>(c): typeof c.value }",
            false,
        ),
        (
            "untyped_after_property",
            "",
            "{ value: number\n read(c): typeof c.value }",
            false,
        ),
        (
            "untyped_after_readonly_property",
            "",
            "{ readonly value: number\n read(c): typeof c.value }",
            false,
        ),
        (
            "untyped_after_method",
            "",
            "{ first(): number\n read(c): typeof c.value }",
            false,
        ),
        ("numeric_method", "", "{ 1(c): typeof c.value }", false),
    ] {
        let before = format!(
            "export {{}};\n{prelude}\nclass C {{\n    value = 7;\n    static same(c: C): {annotation} {{ throw Error('stop'); }}\n}}\nconsole.log('ok');\n"
        );
        let caller = "const untouched = 'caller';\n";
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
            let after = result.as_ref().ok().map(|r| r.rewritten[0].1.clone());
            let error = result.as_ref().err().map(ToString::to_string);
            let selected_unchanged = std::fs::read_to_string(&path).unwrap() == before;
            let caller_unchanged =
                std::fs::read_to_string(ws.root().join("caller.ts")).unwrap() == caller;
            eprintln!(
                "QUERY_BOUNDARY_SOURCE {}",
                serde_json::json!({
                    "name": name, "annotation": annotation, "before": before, "after": after,
                    "error": error, "force": force, "refuse": refuse,
                    "selected_unchanged": selected_unchanged, "caller_unchanged": caller_unchanged,
                    "line": 5, "column": 5
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
            std::fs::write(&path, &before).unwrap();
            std::fs::write(ws.root().join("caller.ts"), caller).unwrap();
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
