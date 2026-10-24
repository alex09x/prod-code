/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::{make_static::make_static_polyglot, replace_inheritance::replace_inheritance_impl};
use prod_code_testkit::{ScriptedGateway, Workspace};
use serde_json::json;

async fn gateway() -> ScriptedGateway {
    ScriptedGateway::start(|method, _| match method {
        "textDocument/references" => json!([]),
        "textDocument/diagnostic" => json!({"kind":"full", "items":[
            {"severity":1,"source":"checker","code":"E1","message":"bad type\nexplanation", "range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}}},
            {"severity":2,"message":"warning", "range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}}},
            {"severity":1,"message":"missing name", "range":{"start":{"line":1,"character":2},"end":{"line":1,"character":3}}}
        ]}),
        _ => serde_json::Value::Null,
    }).await
}

#[tokio::test]
async fn coverage_b2_inheritance_diagnostics_refusal_and_force() {
    let original = "class Parent:\n    def f(self):\n        return 1\n\nclass Child(Parent):\n    pass\n";
    let ws = Workspace::new(&[("classes.py", original)]);
    let gw = gateway().await;
    let expected = vec!["bad type [E1] (classes.py:1:1)", "missing name (classes.py:2:3)"];
    let preview = replace_inheritance_impl(gw.addr(), &ws.root(), &ws.path("classes.py"), "Child", None, None, None, false, false, None).await.unwrap();
    assert_eq!(preview.diagnostics, expected);
    assert_eq!(preview.forwarded_methods, ["f"]);
    assert_eq!(preview.field_name, "_base");
    assert_eq!(ws.read("classes.py"), original);
    let error = replace_inheritance_impl(gw.addr(), &ws.root(), &ws.path("classes.py"), "Child", None, None, None, true, false, None).await.unwrap_err();
    assert_eq!(error.to_string(), "the refactoring produces analyzer errors; nothing was written:\n  bad type [E1] (classes.py:1:1)\n  missing name (classes.py:2:3)");
    assert_eq!(ws.read("classes.py"), original);
    let applied = replace_inheritance_impl(gw.addr(), &ws.root(), &ws.path("classes.py"), "Child", None, None, None, true, true, None).await.unwrap();
    assert_eq!(applied.diagnostics, expected);
    assert_eq!(ws.read("classes.py"), preview.overlays[0].1);
}

#[tokio::test]
async fn coverage_b2_static_diagnostics_refusal_and_force() {
    let original = "class Numbers {\n    add(a: number, b: number): number {\n        return a + b;\n    }\n}\n";
    let ws = Workspace::new(&[("numbers.ts", original)]);
    let gw = gateway().await;
    let diagnostics = vec!["[checker] bad type\nexplanation (numbers.ts:1:1)", "missing name (numbers.ts:2:3)"];
    let preview = make_static_polyglot(gw.addr(), &ws.root(), &ws.path("numbers.ts"), Some("Numbers"), "add", false, false).await.unwrap();
    assert_eq!(preview.diagnostics, diagnostics);
    assert_eq!(preview.owner, "Numbers");
    assert_eq!(preview.receiver, "this");
    assert_eq!(preview.rewritten_calls, 0);
    assert_eq!(ws.read("numbers.ts"), original);
    let error = make_static_polyglot(gw.addr(), &ws.root(), &ws.path("numbers.ts"), Some("Numbers"), "add", true, false).await.unwrap_err();
    assert_eq!(error.to_string(), "the change does not compile (2 error(s)); nothing was written. Pass `force: true` to write it anyway:\n  [checker] bad type\nexplanation (numbers.ts:1:1)\n  missing name (numbers.ts:2:3)");
    assert_eq!(ws.read("numbers.ts"), original);
    let applied = make_static_polyglot(gw.addr(), &ws.root(), &ws.path("numbers.ts"), Some("Numbers"), "add", true, true).await.unwrap();
    assert_eq!(applied.diagnostics, diagnostics);
    assert_eq!(ws.read("numbers.ts"), original.replace("    add(", "    static add("));
}
