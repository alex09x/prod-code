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
use prod_code_mcp::{
    encapsulate_field::{encapsulate, encapsulate_field_go, encapsulate_polyglot},
    tools::execute_tool,
};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};

#[tokio::test]
async fn discovery_failure_propagates_without_writes() {
    let source = "pub struct Config {\n    pub retries: i32,\n}\n";
    let ws = Workspace::new(&[("model.rs", source)]);
    let gw = ScriptedGateway::start(|method, _| match method {
        "textDocument/references" => answers::failure("reference discovery failed"),
        _ => Value::Null,
    })
    .await;
    let err = encapsulate(
        gw.addr(),
        &ws.root(),
        &ws.path("model.rs"),
        2,
        9,
        None,
        true,
        true,
    )
    .await
    .unwrap_err();
    assert!(format!("{err:#}").contains("reference discovery failed"));
    assert_eq!(ws.read("model.rs"), source);
}

#[tokio::test]
async fn unmatched_force_and_compile_mode_refuse_before_writes() {
    let owner = "class User {\n    name = 1;\n}\n";
    let source = "const emoji = '😀'; user.name;\n";
    for verify in ["analyzer", "compile"] {
        let ws = Workspace::new(&[("owner.js", owner), ("main.js", source)]);
        let decl = spot(owner, "name = 1", "name");
        // UTF-16 column 17 is the middle of the emoji surrogate pair, never a name.
        let gw = gateway(
            &ws,
            "User",
            "name",
            ("owner.js", decl),
            &[("main.js", (1, 17))],
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
        assert!(!done.applied);
        assert!(!done.unmatched.is_empty());
        let result = execute_tool(gw.addr(),&ws.root(),"code_encapsulate_field",json!({"path":"owner.js","field":"name","class_name":"User","apply":true,"force":true,"verify":verify})).await;
        assert!(
            result.is_err(),
            "unmatched must prevent the compile gate too"
        );
        assert_eq!(ws.read("owner.js"), owner);
        assert_eq!(ws.read("main.js"), source);
    }
}

#[tokio::test]
async fn ordinary_cpp_unqualified_member_reads_remain_supported() {
    let source = "class Box {\npublic:\n    int count;\n    int value() const { return count; }\n};\nint main() { Box b; b.count = 7; return b.value() == 7 ? 0 : 1; }\n";
    let ws = Workspace::new(&[("main.cpp", source)]);
    let decl = spot(source, "int count;", "count");
    let refs = [
        ("main.cpp", spot(source, "return count;", "count")),
        ("main.cpp", spot(source, "b.count = 7", "count")),
    ];
    let gw = gateway(&ws, "Box", "count", ("main.cpp", decl), &refs).await;
    let done = encapsulate_polyglot(
        gw.addr(),
        &ws.root(),
        &ws.path("main.cpp"),
        Some("Box"),
        "count",
        None,
        true,
        false,
    )
    .await
    .unwrap();
    assert!(done.applied, "{:?}", done.unmatched);
    assert!(ws.read("main.cpp").contains("return count_;"));
    assert!(
        run(&ws, "c++", &["main.cpp", "-o", "probe"])
            .status
            .success()
    );
    assert!(run(&ws, "./probe", &[]).status.success());
}

#[test]
fn go_exported_and_tagged_fields_still_refuse() {
    for (source, field, reason) in [
        (
            "type Box struct {\n    Count int\n}\n",
            "Count",
            "exported Go field",
        ),
        (
            "type Box struct {\n    count int `json:\"count\"`\n}\n",
            "count",
            "tagged Go field",
        ),
    ] {
        let err = encapsulate_field_go(source, Some("Box"), field).unwrap_err();
        assert!(err.to_string().contains(reason));
    }
}

#[tokio::test]
async fn python_multiple_true_references_share_a_line_with_unrelated_text() {
    let source = "class Box:\n    def __init__(self):\n        self.count: int = 2\nb = Box()\nc = type('Other', (), {'count': 9})()\nquoted = 'b.count'\nassert b.count + b.count == 4 and c.count == 9 and quoted == 'b' + chr(46) + 'count'\nprint('python=4 unrelated=9')\n";
    let ws = Workspace::new(&[("main.py", source)]);
    let decl = spot(source, "self.count:", "count");
    let refs = [
        ("main.py", decl),
        ("main.py", spot(source, "count + b.count", "count")),
        ("main.py", spot(source, "count == 4", "count")),
    ];
    let gw = gateway(&ws, "Box", "count", ("main.py", decl), &refs).await;
    let done = encapsulate_polyglot(
        gw.addr(),
        &ws.root(),
        &ws.path("main.py"),
        Some("Box"),
        "count",
        None,
        true,
        false,
    )
    .await
    .unwrap();
    assert!(done.applied, "{:?}", done.unmatched);
    assert!(run(&ws, "python3", &["main.py"]).status.success());
    assert!(ws.read("main.py").contains("c.count == 9"));
    assert!(ws.read("main.py").contains("'b.count'"));
}

/// A Python attribute read on the right can run a property getter that rebinds the receiver
/// before the assignment resolves its target, so it keeps the staged form; a literal keeps the
/// direct setter call (#1406).
#[tokio::test]
async fn python_attribute_values_keep_their_evaluation_order() {
    let source = "class Box:\n    def __init__(self):\n        self.count = 0\nold = Box()\nnew = Box()\nb = old\nclass Trigger:\n    @property\n    def value(self):\n        global b\n        b = new\n        return 7\ntrigger = Trigger()\nb.count = trigger.value\nb.count = 3\nprint(old.count, new.count)\n";
    let ws = Workspace::new(&[("main.py", source)]);
    let before = run(&ws, "python3", &["main.py"]);
    assert_eq!(String::from_utf8_lossy(&before.stdout), "0 3\n");
    let decl = spot(source, "self.count =", "count");
    let refs = [
        ("main.py", decl),
        ("main.py", spot(source, "count = trigger", "count")),
        ("main.py", spot(source, "count = 3", "count")),
        ("main.py", spot(source, "count, new", "count")),
        ("main.py", spot(source, "count)\n", "count")),
    ];
    let gw = gateway(&ws, "Box", "count", ("main.py", decl), &refs).await;
    let done = encapsulate_polyglot(
        gw.addr(),
        &ws.root(),
        &ws.path("main.py"),
        Some("Box"),
        "count",
        None,
        true,
        false,
    )
    .await
    .unwrap();
    assert!(done.applied, "{:?}", done.unmatched);
    let after = run(&ws, "python3", &["main.py"]);
    assert_eq!(
        String::from_utf8_lossy(&after.stdout),
        "0 3\n",
        "{}",
        ws.read("main.py")
    );
    let text = ws.read("main.py");
    assert!(!text.contains("b.set_count(trigger.value)"), "{text}");
    assert!(text.contains("b.set_count(3)"), "{text}");
}
