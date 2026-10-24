/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */
use prod_code_mcp::replace_constructor::{replace_constructor_impl, ReplaceMode};
use prod_code_mcp::inline_parameter::inline_parameter_polyglot;
use prod_code_testkit::{answers, ScriptedGateway, Workspace};

async fn gateway(errors: bool) -> ScriptedGateway {
    ScriptedGateway::start(move |method, _| match method {
        "textDocument/diagnostic" if errors => answers::error_at(1, 1, "E1", "bad type\nmore detail"),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }).await
}

#[tokio::test]
async fn coverage_constructor_diagnostics_and_force() {
    let source = "struct Item { x: i32 }\nfn run() { let _ = Item { x: 7 }; }\n";
    for (apply, force) in [(false, false), (true, false), (true, true)] {
        let ws = Workspace::new(&[("input.rs", source)]);
        let file = ws.root().join("input.rs");
        let gw = gateway(true).await;
        let result = replace_constructor_impl(gw.addr(), ws.root(), &file, "Item", ReplaceMode::Factory, None, apply, force, None).await;
        if apply && !force {
            assert_eq!(result.unwrap_err().to_string(), "the refactoring produces 1 diagnostic error(s); nothing was written. Pass `force: true` to bypass:\n  bad type [E1] (input.rs:1:1)");
            assert_eq!(std::fs::read_to_string(&file).unwrap(), source);
        } else {
            let result = result.unwrap();
            assert_eq!(result.diagnostics, vec!["bad type [E1] (input.rs:1:1)"]);
            assert_eq!(result.target_name, "new");
            assert_eq!(result.instantiations_rewritten, 1);
            assert_eq!(result.applied, apply);
            assert_eq!(result.rewritten[0].1, "struct Item { x: i32 }\n\nimpl Item {\n    fn new(x: i32) -> Self {\n        Self {\n            x,\n        }\n    }\n}\nfn run() { let _ = Item::new(7); }\n");
            assert_eq!(std::fs::read_to_string(&file).unwrap(), if apply { &result.rewritten[0].1 } else { source });
        }
    }
}

#[tokio::test]
async fn coverage_constructor_cpp_builder_placement() {
    let ws = Workspace::new(&[("input.cpp", "struct Item {\nint x;\n};\n")]);
    let file = ws.root().join("input.cpp");
    let gw = gateway(false).await;
    let result = replace_constructor_impl(gw.addr(), ws.root(), &file, "Item", ReplaceMode::Builder, None, false, false, None).await.unwrap();
    assert_eq!(result.target_name, "ItemBuilder");
    assert_eq!(result.declared_fields, vec!["x: int"]);
    assert_eq!(result.instantiations_rewritten, 0);
    assert!(result.rewritten[0].1.starts_with("struct Item {\nint x;\n};\n\nclass ItemBuilder"), "{}", result.rewritten[0].1);
}

#[tokio::test]
async fn coverage_inline_selection_refusals() {
    let source = "function work(x: number, y: number) {\nreturn x + y;\n}\nwork(1, 2);\n";
    let ws = Workspace::new(&[("input.ts", source)]);
    let file = ws.root().join("input.ts");
    let gw = gateway(false).await;
    for (line, col, parameter, error) in [
        (None, None, None, "multiple parameters in `work`; specify which parameter to inline using `parameter`"),
        (None, None, Some("missing"), "parameter `missing` not found in `work`"),
        (Some(1), Some(1), None, "the selected position is not inside a parameter name"),
        (Some(99), Some(99), None, "the parameter position is not inside the file"),
    ] {
        assert_eq!(inline_parameter_polyglot(gw.addr(), ws.root(), &file, line, col, Some("work"), parameter, false, false).await.unwrap_err().to_string(), error);
    }
    let result = inline_parameter_polyglot(gw.addr(), ws.root(), &file, Some(1), Some(15), Some("work"), None, false, false).await.unwrap();
    assert_eq!((result.parameter.as_str(), result.value.as_str(), result.rewritten_calls), ("x", "1", 1));
    assert_eq!(result.rewritten[0].1, "function work(y: number) {\nconst x: number = 1;\nreturn x + y;\n}\nwork(2);\n");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), source);
}

#[tokio::test]
async fn coverage_inline_diagnostics_and_force() {
    let source = "func work(x, y int) {\n\tprintln(x, y)\n}\nfunc run() { work(1, 2) }\n";
    for (apply, force) in [(false, false), (true, false), (true, true)] {
        let ws = Workspace::new(&[("input.go", source)]);
        let file = ws.root().join("input.go");
        let gw = gateway(true).await;
        let result = inline_parameter_polyglot(gw.addr(), ws.root(), &file, None, None, Some("work"), Some("y"), apply, force).await;
        if apply && !force {
            assert_eq!(result.unwrap_err().to_string(), "the change does not compile (1 error(s)); nothing was written. Pass `force: true` to write it anyway:\n  bad type [E1] (input.go:1:1)");
            assert_eq!(std::fs::read_to_string(&file).unwrap(), source);
        } else {
            let result = result.unwrap();
            assert_eq!(result.diagnostics, vec!["bad type [E1] (input.go:1:1)"]);
            assert_eq!(result.rewritten_calls, 1);
            assert_eq!(result.applied, apply);
            assert_eq!(result.rewritten[0].1, "func work(x int) {\n\tconst y = 2\n\tprintln(x, y)\n}\nfunc run() { work(1) }\n");
        }
    }
}
