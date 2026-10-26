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
use prod_code_mcp::encapsulate_field::encapsulate_polyglot;
use prod_code_testkit::Workspace;

const OWNER: &str = "class User {\n    name;\n    constructor() { this.name = function() { return this === user; }; }\n}\nconst user = new User();\n";

async fn callable_case(body: &str) {
    let ws = Workspace::new(&[("owner.js", OWNER), ("main.js", body)]);
    ws.write("probe.js", &format!("{OWNER}{body}"));
    assert!(run(&ws, "node", &["probe.js"]).status.success());
    std::fs::remove_file(ws.path("probe.js")).unwrap();
    let decl = spot(OWNER, "name;", "name");
    let refs = [
        ("owner.js", spot(OWNER, "this.name =", "name")),
        ("main.js", spot(body, "user.name();", "name")),
    ];
    let gw = gateway(&ws, "User", "name", ("owner.js", decl), &refs).await;
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
        "REPORT applied={} unmatched={:?}\n",
        done.applied, done.unmatched
    ));
    emit(format!(
        "EMITTED owner.js:\n{}\nEMITTED main.js:\n{}\n",
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
        "real JS behavior must remain unchanged"
    );
    assert!(
        !done.applied,
        "callable field must explicitly refuse before any writes, even force"
    );
    assert!(!done.unmatched.is_empty());
    assert_eq!(ws.read("owner.js"), OWNER);
    assert_eq!(ws.read("main.js"), body);
}

#[tokio::test]
async fn callable_identity_cannot_be_substituted_by_unrelated_property() {
    callable_case("function demo(user, order) { user.name(); return order.name; }\nconst order = {name: 'order'};\nif (demo(user, order) !== 'order') throw Error('owner changed');\nconsole.log('order');\n").await;
}

#[tokio::test]
async fn callable_identity_cannot_be_substituted_by_string() {
    callable_case("function demo(user) { if (false) user.name(); return \"user.name\"; }\nconst expected = 'user' + String.fromCharCode(46) + 'name';\nif (demo(user) !== expected) throw Error('string changed');\nconsole.log(expected);\n").await;
}

#[tokio::test]
async fn supported_owner_and_external_references_preserve_identity_and_unicode() {
    let owner = "class User {\n    name = 2;\n    value() { return this.name; }\n}\nclass Other {\n    name = 99;\n    value() { return this.name; }\n}\nconst user = new User();\nconst other = new Other();\nconst quoted = 'user.name'; // this.name\nconst emoji = '😀'; user.name = user.name + 1;\nif (user.name + user.name !== 6 || other.name !== 99 || quoted !== 'user' + String.fromCharCode(46) + 'name') throw Error('identity');\n";
    let external = "const mark = '😀'; user.name = user.name + 2 // user.name\nif (user.name + user.name !== 10 || user.value() !== 5 || other.value() !== 99) throw Error('value');\nconsole.log('identity=5 unrelated=99 Unicode=😀');\n";
    let ws = Workspace::new(&[("owner.js", owner), ("main.js", external)]);
    let decl = spot(owner, "name = 2", "name");
    let refs = [
        (
            "owner.js",
            spot(owner, "this.name; }\n}\nclass Other", "name"),
        ),
        ("owner.js", spot(owner, "name = user.name + 1", "name")),
        ("owner.js", spot(owner, "name + 1", "name")),
        ("owner.js", spot(owner, "name + user.name !== 6", "name")),
        ("owner.js", spot(owner, "name !== 6", "name")),
        ("main.js", spot(external, "name = user.name + 2", "name")),
        ("main.js", spot(external, "name + 2", "name")),
        ("main.js", spot(external, "name + user.name !== 10", "name")),
        ("main.js", spot(external, "name !== 10", "name")),
    ];
    let gw = gateway(&ws, "User", "name", ("owner.js", decl), &refs).await;
    let preview = encapsulate_polyglot(
        gw.addr(),
        &ws.root(),
        &ws.path("owner.js"),
        Some("User"),
        "name",
        None,
        false,
        false,
    )
    .await
    .unwrap();
    assert!(!preview.applied);
    assert_eq!(ws.read("owner.js"), owner);
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
    assert!(done.applied, "{:?}", done.unmatched);
    assert_eq!((done.reads, done.writes, done.left_in_file), (6, 2, 1));
    let emitted = format!("{}\n{}", ws.read("owner.js"), ws.read("main.js"));
    emit(format!("EMITTED JS:\n{emitted}\n"));
    ws.write("probe.js", &emitted);
    let run = run(&ws, "node", &["probe.js"]);
    assert!(run.status.success());
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "identity=5 unrelated=99 Unicode=😀\n"
    );
    assert!(
        ws.read("owner.js")
            .contains("value() { return this.name; }\n}\nconst user")
    );
    assert!(ws.read("owner.js").contains("'user.name'; // this.name"));
}

#[tokio::test]
async fn callable_in_owner_file_refuses_without_changing_other_owners_or_strings() {
    for body in [
        "function demo(user, order) { user.name(); return order.name; }\nconst order = {name: 'order'};\nif (demo(user, order) !== 'order') throw Error('owner changed');\n",
        "function demo(user) { if (false) user.name(); return \"user.name\"; }\nif (demo(user) !== 'user' + String.fromCharCode(46) + 'name') throw Error('string changed');\n",
    ] {
        let source = format!("{OWNER}{body}");
        let ws = Workspace::new(&[("main.js", &source)]);
        let decl = spot(&source, "    name;", "name");
        let refs = [
            ("main.js", spot(&source, "this.name =", "name")),
            ("main.js", spot(&source, "user.name();", "name")),
        ];
        let gw = gateway(&ws, "User", "name", ("main.js", decl), &refs).await;
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
        assert!(!done.applied);
        assert!(done.unmatched.iter().any(|s| s.contains("callable field")));
        assert_eq!(ws.read("main.js"), source);
        assert!(run(&ws, "node", &["main.js"]).status.success());
    }
}

#[tokio::test]
async fn callable_wrappers_and_comments_cannot_bypass_refusal() {
    for expression in [
        "user.name?.()",
        "(user.name)()",
        "user.name /* receiver */ ()",
        "user.name`tag`",
    ] {
        let source = format!("{OWNER}\nif ({expression} !== true) throw Error('receiver');\n");
        let ws = Workspace::new(&[("main.js", &source)]);
        let decl = spot(&source, "    name;", "name");
        let refs = [
            ("main.js", spot(&source, "this.name =", "name")),
            ("main.js", spot(&source, expression, "name")),
        ];
        let gw = gateway(&ws, "User", "name", ("main.js", decl), &refs).await;
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
        assert!(!done.applied, "{expression}");
        assert!(done.unmatched.iter().any(|s| s.contains("callable field")));
        assert_eq!(ws.read("main.js"), source);
        assert!(run(&ws, "node", &["main.js"]).status.success());
    }
}
