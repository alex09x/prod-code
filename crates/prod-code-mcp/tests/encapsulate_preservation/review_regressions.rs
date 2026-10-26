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
use prod_code_mcp::encapsulate_field::{encapsulate, encapsulate_polyglot};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::Value;

#[tokio::test]
async fn swift_newline_statement_rewrites_but_nested_assignment_refuses() {
    let owner = "class Box {\n    var count: Int\n    init() { self.count = 1 }\n}\n";
    for (body, supported) in [
        ("let b = Box()\nb.count = 2\nprint(b.count)\n", true),
        ("let b = Box()\nconsume(\n    b.count = 2\n)\n", false),
        ("let b = Box()\nlet value =\n    b.count = 2\n", false),
    ] {
        let ws = Workspace::new(&[("owner.swift", owner), ("main.swift", body)]);
        let mut refs = vec![
            ("owner.swift", spot(owner, "self.count =", "count")),
            ("main.swift", spot(body, "b.count =", "count")),
        ];
        if supported {
            refs.push(("main.swift", spot(body, "print(b.count)", "count")));
        }
        let gw = gateway(
            &ws,
            "Box",
            "count",
            ("owner.swift", spot(owner, "var count:", "count")),
            &refs,
        )
        .await;
        let plan = encapsulate_polyglot(
            gw.addr(),
            &ws.root(),
            &ws.path("owner.swift"),
            Some("Box"),
            "count",
            None,
            true,
            true,
        )
        .await
        .unwrap();
        emit(format!(
            "SWIFT supported={supported} applied={} unmatched={:?}\nEMITTED owner.swift:\n{}\nEMITTED main.swift:\n{}\n",
            plan.applied,
            plan.unmatched,
            ws.read("owner.swift"),
            ws.read("main.swift")
        ));
        assert_eq!(
            plan.applied, supported,
            "ordinary newline statement stays supported"
        );
        if supported {
            assert!(plan.unmatched.is_empty());
            assert!(
                ws.read("main.swift")
                    .contains("b.setCount(2)\nprint(b.getCount())")
            );
            assert!(ws.read("owner.swift").contains("self._count = 1"));
        } else {
            assert!(
                plan.unmatched
                    .iter()
                    .any(|s| s.contains("assignment value"))
            );
            assert_eq!(ws.read("owner.swift"), owner);
            assert_eq!(ws.read("main.swift"), body);
        }
    }
}

#[tokio::test]
async fn javascript_unicode_receivers_keep_positions_and_runtime() {
    let owner = "class User {\n    name = 1;\n}\nconst 丹 = new User();\n";
    let body = "console.log(丹.name);\n丹.name = 丹.name + 1;\nif (丹.name !== 2) throw Error('Unicode receiver');\nconst café = 丹;\nif ((café.name) !== 2) throw Error('mixed receiver');\nconsole.log('Unicode receiver=2');\n";
    let ws = Workspace::new(&[("owner.js", owner), ("main.js", body)]);
    ws.write("probe.js", &format!("{owner}{body}"));
    assert!(run(&ws, "node", &["probe.js"]).status.success());
    std::fs::remove_file(ws.path("probe.js")).unwrap();
    let refs = [
        "丹.name)",
        "丹.name =",
        "丹.name +",
        "丹.name !==",
        "café.name)",
    ]
    .map(|anchor| ("main.js", spot(body, anchor, "name")));
    let gw = gateway(
        &ws,
        "User",
        "name",
        ("owner.js", spot(owner, "name = 1", "name")),
        &refs,
    )
    .await;
    let done = encapsulate_polyglot(
        gw.addr(),
        &ws.root(),
        &ws.path("owner.js"),
        Some("User"),
        "name",
        None,
        true,
        false,
    )
    .await
    .unwrap();
    emit(format!(
        "UNICODE applied={} unmatched={:?}\nEMITTED owner.js:\n{}\nEMITTED main.js:\n{}\n",
        done.applied,
        done.unmatched,
        ws.read("owner.js"),
        ws.read("main.js")
    ));
    assert!(done.applied);
    assert!(done.unmatched.is_empty());
    assert!(ws.read("main.js").contains("丹.setName(丹.getName() + 1)"));
    ws.write(
        "probe.js",
        &format!("{}{}", ws.read("owner.js"), ws.read("main.js")),
    );
    assert!(run(&ws, "node", &["probe.js"]).status.success());
}

#[tokio::test]
async fn rust_type_alias_collision_refuses_before_proposing_or_writing() {
    let source = "mod inner {\n    pub struct Config {\n        pub retries: i32,\n    }\n    pub fn new() -> Config { Config { retries: 3 } }\n    type Alias = Config;\n    impl Alias {\n        pub fn retries(&self) -> i32 { self.retries }\n    }\n}\nfn main() {\n    let cfg = inner::new();\n    assert_eq!(cfg.retries(), 3);\n    println!(\"type alias accessor=3\");\n}\n";
    let ws = Workspace::new(&[("main.rs", source)]);
    rust_runtime(&ws);
    let decl = spot(source, "pub retries:", "retries");
    let locations = answers::locations(
        &ws.path("main.rs"),
        &[
            spot(source, "Config { retries:", "retries"),
            spot(source, "self.retries", "retries"),
        ],
    );
    let alias_definition =
        answers::locations(&ws.path("main.rs"), &[spot(source, "type Alias", "Alias")]);
    let nominal_definition = answers::locations(
        &ws.path("main.rs"),
        &[spot(source, "struct Config", "Config")],
    );
    let impl_owner = spot(source, "impl Alias", "Alias");
    let alias_rhs = spot(source, "= Config;", "Config");
    let gw = ScriptedGateway::start(move |method, params| match method {
        "textDocument/references" => locations.clone(),
        "textDocument/definition" => {
            let pos = &params["position"];
            if pos["line"] == impl_owner.0 - 1 && pos["character"] == impl_owner.1 - 1 {
                alias_definition.clone()
            } else {
                assert_eq!(pos["line"], alias_rhs.0 - 1);
                assert_eq!(pos["character"], alias_rhs.1 - 1);
                nominal_definition.clone()
            }
        }
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await;
    for apply in [false, true] {
        let result = encapsulate(
            gw.addr(),
            &ws.root(),
            &ws.path("main.rs"),
            decl.0,
            decl.1,
            None,
            apply,
            true,
        )
        .await;
        emit(format!("TYPE ALIAS apply={apply} result={result:?}\n"));
        if let Ok(plan) = &result {
            ws.write("candidate.rs", &plan.rewritten[0].1);
            emit(format!(
                "EMITTED candidate.rs:\n{}\n",
                ws.read("candidate.rs")
            ));
            assert!(
                run(
                    &ws,
                    "rustc",
                    &["--edition=2024", "candidate.rs", "-o", "candidate",]
                )
                .status
                .success(),
                "invalid duplicate accessor proposal must be refused"
            );
        }
        assert!(
            result.is_err(),
            "alias collision must refuse before any writes"
        );
        assert!(format!("{:#}", result.unwrap_err()).contains("already has a `fn retries`"));
        assert_eq!(ws.read("main.rs"), source);
    }
}

#[tokio::test]
async fn rust_type_alias_for_distinct_owner_still_allows_encapsulation() {
    let source = "mod inner {\n    pub struct Config {\n        pub retries: i32,\n    }\n    pub fn new() -> Config { Config { retries: 3 } }\n    pub mod child {\n        pub struct Other;\n        type Alias = Other;\n        impl Alias {\n            pub fn retries(&self) -> i32 { 99 }\n        }\n    }\n}\nfn main() {\n    let cfg = inner::new();\n    assert_eq!(cfg.retries, 3);\n    assert_eq!(inner::child::Other.retries(), 99);\n}\n";
    let ws = Workspace::new(&[("main.rs", source)]);
    rust_runtime(&ws);
    let decl = spot(source, "pub retries:", "retries");
    let locations = answers::locations(
        &ws.path("main.rs"),
        &[
            spot(source, "Config { retries:", "retries"),
            spot(source, "cfg.retries,", "retries"),
        ],
    );
    let alias_definition =
        answers::locations(&ws.path("main.rs"), &[spot(source, "type Alias", "Alias")]);
    let nominal_definition = answers::locations(
        &ws.path("main.rs"),
        &[spot(source, "struct Other", "Other")],
    );
    let impl_owner = spot(source, "impl Alias", "Alias");
    let alias_rhs = spot(source, "= Other;", "Other");
    let gw = ScriptedGateway::start(move |method, params| match method {
        "textDocument/references" => locations.clone(),
        "textDocument/definition" => {
            let pos = &params["position"];
            if pos["line"] == impl_owner.0 - 1 && pos["character"] == impl_owner.1 - 1 {
                alias_definition.clone()
            } else {
                assert_eq!(pos["line"], alias_rhs.0 - 1);
                assert_eq!(pos["character"], alias_rhs.1 - 1);
                nominal_definition.clone()
            }
        }
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await;
    let done = encapsulate(
        gw.addr(),
        &ws.root(),
        &ws.path("main.rs"),
        decl.0,
        decl.1,
        None,
        true,
        false,
    )
    .await
    .unwrap();
    assert!(done.applied);
    assert!(ws.read("main.rs").contains("cfg.retries(), 3"));
    rust_runtime(&ws);
}
