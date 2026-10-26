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
use prod_code_testkit::Workspace;

#[tokio::test]
async fn cpp_template_rhs_keeps_argument_commas_and_expression_boundaries() {
    let owner = "class Box {\npublic:\n    int count;\n};\n";
    let caller = "#include \"box.hpp\"\n#include <utility>\ntemplate<class A, class B> int choose(int a, int b) { return a + b; }\nint main() {\n    Box b;\n    b.count = choose<int, int>(1, 2);\n    if (b.count != 3) return 1;\n    b.count = choose<std::pair<int, int>, int>(3, 4);\n    if (b.count != 7) return 2;\n    b.count = 1 < 2 ? 9 : 8;\n    if (b.count != 9) return 3;\n    b.count = 2 << 1;\n    if (b.count != 4) return 4;\n    return 0;\n}\n";
    let ws = Workspace::new(&[("box.hpp", owner), ("main.cpp", caller)]);
    assert!(
        run(&ws, "c++", &["main.cpp", "-o", "probe"])
            .status
            .success()
    );
    assert!(run(&ws, "./probe", &[]).status.success());
    let refs = [
        "count = choose<int",
        "count != 3",
        "count = choose<std",
        "count != 7",
        "count = 1 <",
        "count != 9",
        "count = 2 <<",
        "count != 4",
    ]
    .map(|anchor| ("main.cpp", spot(caller, anchor, "count")));
    let gw = gateway(
        &ws,
        "Box",
        "count",
        ("box.hpp", spot(owner, "int count;", "count")),
        &refs,
    )
    .await;
    let done = encapsulate_polyglot(
        gw.addr(),
        &ws.root(),
        &ws.path("box.hpp"),
        Some("Box"),
        "count",
        None,
        true,
        true,
    )
    .await
    .unwrap();
    emit(format!(
        "CPP REPORT applied={} unmatched={:?}\nEMITTED box.hpp:\n{}\nEMITTED main.cpp:\n{}\n",
        done.applied,
        done.unmatched,
        ws.read("box.hpp"),
        ws.read("main.cpp")
    ));
    assert!(
        run(&ws, "c++", &["main.cpp", "-o", "probe"])
            .status
            .success()
    );
    assert!(run(&ws, "./probe", &[]).status.success());
    assert!(done.applied, "{:?}", done.unmatched);
    assert!(ws.read("main.cpp").contains("))(choose<int, int>(1, 2));"));
}

#[tokio::test]
async fn cpp_ambiguous_cast_and_comma_refuses_without_writes() {
    let owner = "class Box {\npublic:\n    int count;\n};\n";
    let caller = "#include \"box.hpp\"\nint main() {\n    Box b; int a = 0, c = 3, d = 2;\n    b.count = a < int(1), c > (d);\n    return b.count == 1 ? 0 : 1;\n}\n";
    let ws = Workspace::new(&[("box.hpp", owner), ("main.cpp", caller)]);
    assert!(
        run(&ws, "c++", &["main.cpp", "-o", "probe"])
            .status
            .success()
    );
    assert!(run(&ws, "./probe", &[]).status.success());
    let refs =
        ["count = a", "count == 1"].map(|anchor| ("main.cpp", spot(caller, anchor, "count")));
    let gw = gateway(
        &ws,
        "Box",
        "count",
        ("box.hpp", spot(owner, "int count;", "count")),
        &refs,
    )
    .await;
    let result = encapsulate_polyglot(
        gw.addr(),
        &ws.root(),
        &ws.path("box.hpp"),
        Some("Box"),
        "count",
        None,
        true,
        true,
    )
    .await;
    emit(format!(
        "AMBIGUOUS CPP RESULT {result:?}\nEMITTED box.hpp:\n{}\nEMITTED main.cpp:\n{}\n",
        ws.read("box.hpp"),
        ws.read("main.cpp")
    ));
    assert!(
        result.is_err(),
        "an ambiguous comma must not be guessed to belong to a template"
    );
    assert_eq!(ws.read("box.hpp"), owner);
    assert_eq!(ws.read("main.cpp"), caller);
}

#[tokio::test]
async fn javascript_consumed_assignment_refuses_before_writes_even_force() {
    for (body, anchors) in [
        (
            "a.name = b.name = 2;\nif (a.name !== 2 || b.name !== 2) throw Error('chain value');",
            vec!["a.name =", "b.name =", "a.name !==", "b.name !=="],
        ),
        (
            "const value = (a.name = 2);\nif (value !== 2 || a.name !== 2) throw Error('initializer value');",
            vec!["a.name =", "a.name !=="],
        ),
        (
            "function assign() { return a.name = 2; }\nif (assign() !== 2 || a.name !== 2) throw Error('return value');",
            vec!["a.name =", "a.name !=="],
        ),
        (
            "function check(x) { if (x !== 2) throw Error('argument value'); }\ncheck(a.name = 2);",
            vec!["a.name ="],
        ),
        (
            "(a.name = 2) && (b.name = 3);\nif (b.name !== 3) throw Error('condition value');",
            vec!["a.name =", "b.name =", "b.name !=="],
        ),
    ] {
        let owner =
            "class User {\n    name = 1;\n}\nconst a = new User();\nconst b = new User();\n";
        let ws = Workspace::new(&[("owner.js", owner), ("main.js", body)]);
        ws.write("probe.js", &format!("{owner}{body}"));
        assert!(run(&ws, "node", &["probe.js"]).status.success());
        std::fs::remove_file(ws.path("probe.js")).unwrap();
        let refs: Vec<_> = anchors
            .iter()
            .map(|anchor| ("main.js", spot(body, anchor, "name")))
            .collect();
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
            true,
        )
        .await
        .unwrap();
        emit(format!(
            "ASSIGNMENT REPORT applied={} unmatched={:?}\nEMITTED owner.js:\n{}\nEMITTED main.js:\n{}\n",
            done.applied,
            done.unmatched,
            ws.read("owner.js"),
            ws.read("main.js")
        ));
        ws.write(
            "probe.js",
            &format!("{}{}", ws.read("owner.js"), ws.read("main.js")),
        );
        let actual = run(&ws, "node", &["probe.js"]);
        assert!(
            actual.status.success(),
            "assignment result must be preserved"
        );
        assert!(
            !done.applied,
            "consumed assignment must refuse before writes"
        );
        assert!(
            done.unmatched
                .iter()
                .any(|s| s.contains("assignment value"))
        );
        assert_eq!(ws.read("owner.js"), owner);
        assert_eq!(ws.read("main.js"), body);
    }
}

#[tokio::test]
async fn raw_module_privacy_rewrites_parent_and_sibling_only() {
    let source = "mod r#type {\n    pub struct Config {\n        pub retries: i32,\n    }\n    impl Config {\n        pub fn new() -> Self { Self { retries: 3 } }\n        pub fn direct(&self) -> i32 { self.retries }\n    }\n    pub mod child {\n        pub fn direct(c: &super::Config) -> i32 { c.retries }\n    }\n}\nmod sibling {\n    pub fn read(c: &super::r#type::Config) -> i32 { c.retries }\n}\nfn main() {\n    let mut cfg = r#type::Config::new();\n    cfg.retries = cfg.retries + 1;\n    assert_eq!(cfg.retries, 4);\n    assert_eq!(cfg.direct(), 4);\n    assert_eq!(r#type::child::direct(&cfg), 4);\n    assert_eq!(sibling::read(&cfg), 4);\n    println!(\"raw privacy=4 internal=4 descendant=4 sibling=4\");\n}\n";
    let ws = Workspace::new(&[("main.rs", source)]);
    rust_runtime(&ws);
    let decl = spot(source, "pub retries:", "retries");
    let refs = [
        "Self { retries:",
        "self.retries }",
        "c.retries }\n    }",
        "c.retries }\n}",
        "retries = cfg",
        "retries + 1",
        "retries, 4",
    ]
    .map(|anchor| ("main.rs", spot(source, anchor, "retries")));
    let gw = gateway(&ws, "Config", "retries", ("main.rs", decl), &refs).await;
    let done = encapsulate(
        gw.addr(),
        &ws.root(),
        &ws.path("main.rs"),
        decl.0,
        decl.1,
        None,
        true,
        true,
    )
    .await
    .unwrap();
    emit(format!(
        "RAW PRIVACY REPORT applied={} unmatched={:?}\n",
        done.applied, done.unmatched
    ));
    rust_runtime(&ws);
    assert!(done.applied);
    assert_eq!((done.reads, done.writes, done.left_in_file), (3, 1, 3));
    assert!(ws.read("main.rs").contains("self.retries }"));
    assert!(ws.read("main.rs").contains("c.retries }\n    }"));
}

#[tokio::test]
async fn javascript_internal_semantic_slots_keep_whitespace_and_callable_receiver() {
    let source = "class User {\n    name = 2;\n    value() { return this .name; }\n    update() { this .name = this .name + 1; }\n}\nconst user = new User();\nuser.update();\nif (user.value() !== 3) throw Error('internal read/write');\nconsole.log('internal=3');\n";
    let ws = Workspace::new(&[("main.js", source)]);
    assert!(run(&ws, "node", &["main.js"]).status.success());
    let refs = ["return this .name", "name = this .name", "name + 1"]
        .map(|anchor| ("main.js", spot(source, anchor, "name")));
    let gw = gateway(
        &ws,
        "User",
        "name",
        ("main.js", spot(source, "name = 2", "name")),
        &refs,
    )
    .await;
    let done = encapsulate_polyglot(
        gw.addr(),
        &ws.root(),
        &ws.path("main.js"),
        Some("User"),
        "name",
        None,
        true,
        true,
    )
    .await
    .unwrap();
    emit(format!(
        "INTERNAL REPORT applied={} unmatched={:?}\nEMITTED main.js:\n{}\n",
        done.applied,
        done.unmatched,
        ws.read("main.js")
    ));
    assert!(run(&ws, "node", &["main.js"]).status.success());
    assert!(done.applied, "{:?}", done.unmatched);
    assert!(ws.read("main.js").contains("return this .#name"));
    assert_eq!(done.left_in_file, 3);
}

#[tokio::test]
async fn javascript_internal_callable_keeps_receiver_and_argument_order() {
    let source = "class User {\n    name;\n    constructor() { this .name = function(a, b) { if (this !== user || a !== 1 || b !== 2) throw Error('receiver/order'); return 3; }; }\n    invoke() { let n = 0; return this .name(++n, ++n); }\n}\nconst user = new User();\nif (user.invoke() !== 3) throw Error('call value');\nconsole.log('receiver/order=3');\n";
    let ws = Workspace::new(&[("main.js", source)]);
    assert!(run(&ws, "node", &["main.js"]).status.success());
    let refs =
        ["this .name =", "this .name(++n"].map(|anchor| ("main.js", spot(source, anchor, "name")));
    let gw = gateway(
        &ws,
        "User",
        "name",
        ("main.js", spot(source, "    name;", "name")),
        &refs,
    )
    .await;
    let done = encapsulate_polyglot(
        gw.addr(),
        &ws.root(),
        &ws.path("main.js"),
        Some("User"),
        "name",
        None,
        true,
        true,
    )
    .await
    .unwrap();
    emit(format!(
        "INTERNAL CALLABLE REPORT applied={} unmatched={:?}\nEMITTED main.js:\n{}\n",
        done.applied,
        done.unmatched,
        ws.read("main.js")
    ));
    assert!(done.applied, "{:?}", done.unmatched);
    assert!(run(&ws, "node", &["main.js"]).status.success());
    assert!(ws.read("main.js").contains("this .#name(++n, ++n)"));
}
