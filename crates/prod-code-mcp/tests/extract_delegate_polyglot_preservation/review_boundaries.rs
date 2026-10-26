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
use prod_code_testkit::Workspace;
use std::{fs, process::Command};

async fn readonly_refusal(source: &str, anchors: &[&str], expected: &str, reason: &str) {
    let ws = Workspace::new(&[("account.ts", source)]);
    let file = ws.path("account.ts");
    let refs = anchors
        .iter()
        .map(|anchor| at(&file, source, anchor, "city"))
        .collect();
    let gw = gateway(&file, source, "Account", "city", "readonly city:", refs).await;
    run(
        &ws,
        "tsc",
        &["--target", "ES2022", "--module", "commonjs", "account.ts"],
    );
    let before = run(&ws, "node", &["account.js"]);
    println!("original TypeScript/Node stdout: {before:?}");
    assert_eq!(before, expected);
    let result = extract(
        &ws,
        &gw,
        "account.ts",
        "Account",
        "city",
        &[],
        "Address",
        "address",
        true,
    )
    .await;
    if result.is_ok() {
        let compile = Command::new("tsc")
            .args(["--target", "ES2022", "--module", "commonjs", "account.ts"])
            .current_dir(ws.root())
            .output()
            .unwrap();
        println!(
            "accepted extraction TypeScript exit {:?}: {}{}",
            compile.status.code(),
            String::from_utf8_lossy(&compile.stdout),
            String::from_utf8_lossy(&compile.stderr)
        );
        if compile.status.success() {
            let node = Command::new("node")
                .arg("account.js")
                .current_dir(ws.root())
                .output()
                .unwrap();
            println!(
                "accepted extraction Node exit {:?}: {}{}",
                node.status.code(),
                String::from_utf8_lossy(&node.stdout),
                String::from_utf8_lossy(&node.stderr)
            );
        }
    }
    let error = result.expect_err("unsafe deferred initialization must refuse before writes");
    assert!(error.to_string().contains(reason), "{error:#}");
    assert_eq!(fs::read_to_string(file).unwrap(), source);
}

#[tokio::test]
async fn readonly_assignment_statement_boundary_refuses_atomically() {
    readonly_refusal(
        "const events: string[] = [];\nclass Account {\n    readonly city: string;\n    constructor(city: string) {\n        this.city = city; events.push('after');\n    }\n}\nconsole.log(new Account('Paris').city, events.join(','));\n",
        &["this.city =", ").city,"],
        "Paris after\n",
        "deferred initializer requires one complete assignment statement",
    )
    .await;
}

#[tokio::test]
async fn readonly_indirect_self_read_refuses_atomically() {
    for rhs in ["this.current() ?? city", "`${this.current() ?? city}`"] {
        let source = format!(
            "class Account {{\n    readonly city: string;\n    constructor(city: string) {{\n        this.city = {rhs};\n    }}\n    current(): string | undefined {{ return this.city; }}\n}}\nconsole.log(new Account('Paris').city);\n"
        );
        readonly_refusal(
            &source,
            &["this.city =", "return this.city", ").city)"],
            "Paris\n",
            "deferred initializer has unproved owner-dependent evaluation",
        )
        .await;
    }
}

#[tokio::test]
async fn control_body_member_calls_refuse_atomically() {
    let owner = "class Account {\n    city = function() { console.log(this.id); return this.id; };\n    id = 7;\n}\nmodule.exports = { Account };\n";
    for statement in [
        "if (true) console.log((a.city)());",
        "if (true) (a.city)();",
        "if /* header */ (true) /* body */ (a.city)();",
        "if (true) (a.city)`tag`;",
        "let once = true; while (once) (a.city)(once = false);",
        "if (false) {} else (a.city)();",
        "do (a.city)(); while (false);",
        "for (let i = 0; i < 1; i++) (a.city)();",
    ] {
        let caller = format!(
            "const {{ Account }} = require('./account');\nconst a = new Account();\n{statement}\n"
        );
        let ws = Workspace::new(&[("account.js", owner), ("main.js", &caller)]);
        let file = ws.path("account.js");
        let gw = gateway(
            &file,
            owner,
            "Account",
            "city",
            "city = function()",
            vec![at(&ws.path("main.js"), &caller, "a.city", "city")],
        )
        .await;
        let before = run(&ws, "node", &["main.js"]);
        assert!(before.starts_with("7\n"), "{statement}: {before:?}");
        println!("original Node {statement}: {before:?}");
        let result = extract(
            &ws,
            &gw,
            "account.js",
            "Account",
            "city",
            &[],
            "Address",
            "address",
            true,
        )
        .await;
        if result.is_ok() {
            println!(
                "accepted extraction Node stdout: {:?}",
                run(&ws, "node", &["main.js"])
            );
        }
        let error = result.expect_err("member call must preserve its receiver or refuse");
        assert!(error.to_string().contains("callable field"), "{error:#}");
        assert_eq!(fs::read_to_string(file).unwrap(), owner);
        assert_eq!(fs::read_to_string(ws.path("main.js")).unwrap(), caller);
        assert_eq!(run(&ws, "node", &["main.js"]), before);
    }
}

#[tokio::test]
async fn readonly_nested_rhs_statements_and_literal_owner_text_remain_supported() {
    let source = "const events: string[] = [];\nclass Account {\n    readonly city: string;\n    constructor(city: string) {\n        this.city = (() => { events.push('this;😀'); return city; })();\n    }\n}\nconsole.log(new Account('Paris').city, events.join(','));\n";
    let ws = Workspace::new(&[("account.ts", source)]);
    let file = ws.path("account.ts");
    let gw = gateway(
        &file,
        source,
        "Account",
        "city",
        "readonly city:",
        vec![
            at(&file, source, "this.city =", "city"),
            at(&file, source, ").city,", "city"),
        ],
    )
    .await;
    let compile = || {
        run(
            &ws,
            "tsc",
            &["--target", "ES2022", "--module", "commonjs", "account.ts"],
        )
    };
    compile();
    let before = run(&ws, "node", &["account.js"]);
    assert_eq!(before, "Paris this;😀\n");
    extract(
        &ws,
        &gw,
        "account.ts",
        "Account",
        "city",
        &[],
        "Address",
        "address",
        true,
    )
    .await
    .unwrap();
    let after = fs::read_to_string(file).unwrap();
    assert!(after.contains("readonly city: string;"), "{after}");
    assert!(
        after.contains("this.address = new Address((() =>"),
        "{after}"
    );
    compile();
    assert_eq!(run(&ws, "node", &["account.js"]), before);
}

#[tokio::test]
async fn scalar_argument_after_chained_call_remains_supported() {
    let source = "class Account {\n    city = 'Paris';\n}\nconst factory = () => value => () => value;\nconst a = new Account();\nconsole.log(factory()(a.city)());\n";
    let ws = Workspace::new(&[("account.js", source)]);
    let file = ws.path("account.js");
    let gw = gateway(
        &file,
        source,
        "Account",
        "city",
        "city = 'Paris'",
        vec![at(&file, source, "a.city", "city")],
    )
    .await;
    let before = run(&ws, "node", &["account.js"]);
    assert_eq!(before, "Paris\n");
    extract(
        &ws,
        &gw,
        "account.js",
        "Account",
        "city",
        &[],
        "Address",
        "address",
        true,
    )
    .await
    .unwrap();
    let after = fs::read_to_string(file).unwrap();
    assert!(after.contains("factory()(a.address.city)()"), "{after}");
    assert_eq!(run(&ws, "node", &["account.js"]), before);
}
