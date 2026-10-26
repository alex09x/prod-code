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
use std::fs;

fn compile_ts(ws: &Workspace) {
    run(
        ws,
        "tsc",
        &["--target", "ES2022", "--module", "commonjs", "account.ts"],
    );
}

#[tokio::test]
async fn parenthesized_tags_preserve_receiver_or_refuse_atomically() {
    let owner = "class Account {\n    city = function() { return this.id; };\n    id = 7;\n}\nmodule.exports = { Account };\n";
    for expression in [
        "(a.city)`x`",
        "((a.city)) /* tag */ `x`",
        "`${(a.city)`x`}`",
    ] {
        let caller = format!(
            "const {{ Account }} = require('./account');\nconst a = new Account();\nconsole.log({expression});\n"
        );
        let ws = Workspace::new(&[("account.js", owner), ("main.js", &caller)]);
        let file = ws.path("account.js");
        let gw = gateway(
            &file,
            owner,
            "Account",
            "city",
            "city = function()",
            vec![at(&ws.path("main.js"), &caller, expression, "city")],
        )
        .await;
        let before = run(&ws, "node", &["main.js"]);
        println!("tag before: {before:?}");
        assert_eq!(before, "7\n");
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
        if let Err(error) = result {
            assert!(error.to_string().contains("callable field"), "{error:#}");
            assert_eq!(fs::read_to_string(file).unwrap(), owner);
            assert_eq!(fs::read_to_string(ws.path("main.js")).unwrap(), caller);
        }
        let after = run(&ws, "node", &["main.js"]);
        println!("tag after: {after:?}");
        assert_eq!(after, before);
        evidence(
            if expression.starts_with('`') {
                "template-tag.js"
            } else if expression.starts_with("((") {
                "nested-tag.js"
            } else {
                "tag.js"
            },
            &caller,
            &fs::read_to_string(ws.path("main.js")).unwrap(),
            &after,
        );
    }
}

#[tokio::test]
async fn readonly_constructor_initialization_keeps_evaluation_point() {
    let source = "const events: string[] = [];\nclass Account {\n    readonly city: string;\n    id = (events.push('field'), 7);\n    constructor(city: string) {\n        events.push('before');\n        this.city = (events.push('assign'), city);\n        events.push('after:' + this.city);\n    }\n}\nconst a = new Account('Paris');\nconsole.log(a.city, a.id, events.join(','));\n";
    let ws = Workspace::new(&[("account.ts", source)]);
    let file = ws.path("account.ts");
    let refs = ["this.city =", "this.city);", "a.city, a.id"]
        .iter()
        .map(|anchor| at(&file, source, anchor, "city"))
        .collect();
    let gw = gateway(&file, source, "Account", "city", "readonly city:", refs).await;
    compile_ts(&ws);
    let before = run(&ws, "node", &["account.js"]);
    println!("readonly before: {before:?}");
    assert_eq!(before, "Paris 7 field,before,assign,after:Paris\n");
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
    let after = fs::read_to_string(&file).unwrap();
    assert!(after.contains("readonly city: string;"), "{after}");
    compile_ts(&ws);
    assert_eq!(run(&ws, "node", &["account.js"]), before);
    evidence("readonly.ts", source, &after, &before);
}

#[tokio::test]
async fn scalar_assertions_and_non_null_reads_remain_supported() {
    let source = "class Account {\n    city = 'Paris';\n}\nfunction echo(value: string) { return () => value; }\nconst a = new Account();\nconsole.log(a.city as string, a.city satisfies string, a.city!, (a.city as string).length, echo(a.city as string)(), echo((a.city satisfies string))());\n";
    let ws = Workspace::new(&[("account.ts", source)]);
    let file = ws.path("account.ts");
    let refs = [
        "a.city as string,",
        "a.city satisfies string,",
        "a.city!",
        "a.city as string).length",
        "a.city as string)()",
        "a.city satisfies string))",
    ]
    .iter()
    .map(|anchor| at(&file, source, anchor, "city"))
    .collect();
    let gw = gateway(&file, source, "Account", "city", "city = 'Paris'", refs).await;
    compile_ts(&ws);
    let before = run(&ws, "node", &["account.js"]);
    println!("scalar before: {before:?}");
    assert_eq!(before, "Paris Paris Paris 5 Paris Paris\n");
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
    compile_ts(&ws);
    assert_eq!(run(&ws, "node", &["account.js"]), before);
    evidence(
        "scalar.ts",
        source,
        &fs::read_to_string(file).unwrap(),
        &before,
    );
}

#[tokio::test]
async fn readonly_declaration_initializer_keeps_order_and_modifier() {
    let source = "const events: string[] = [];\nclass Account {\n    readonly city: string = (events.push('city'), 'Paris');\n    id = (events.push('id'), 7);\n    constructor() { events.push('constructor'); }\n}\nconst a = new Account();\nconsole.log(a.city, a.id, events.join(','));\n";
    let ws = Workspace::new(&[("account.ts", source)]);
    let file = ws.path("account.ts");
    let gw = gateway(
        &file,
        source,
        "Account",
        "city",
        "readonly city:",
        vec![at(&file, source, "a.city, a.id", "city")],
    )
    .await;
    compile_ts(&ws);
    let before = run(&ws, "node", &["account.js"]);
    assert_eq!(before, "Paris 7 city,id,constructor\n");
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
    compile_ts(&ws);
    assert_eq!(run(&ws, "node", &["account.js"]), before);
    evidence("readonly-initializer.ts", source, &after, &before);
}
