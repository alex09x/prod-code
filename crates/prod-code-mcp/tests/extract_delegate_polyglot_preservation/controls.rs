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

#[tokio::test]
async fn external_unicode_crlf_exact_occurrence_and_literal_preservation() {
    let owner = "class Account {\n    city = 3;\n}\nmodule.exports = { Account };\n";
    let caller = "const { Account } = require('./account');\r\nconst a = new Account(), other = { city: 9 };\r\nconsole.log('😀', a.city < other.city, '.city');\r\n";
    let ws = Workspace::new(&[("account.js", owner), ("main.js", caller)]);
    let gw = gateway(
        &ws.path("account.js"),
        owner,
        "Account",
        "city",
        "city = 3",
        vec![at(&ws.path("main.js"), caller, "a.city <", "city")],
    )
    .await;
    let before = run(&ws, "node", &["main.js"]);
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
    let after = fs::read_to_string(ws.path("main.js")).unwrap();
    assert_eq!(after, caller.replace("a.city <", "a.address.city <"));
    assert_eq!(run(&ws, "node", &["main.js"]), before);
    evidence("external.js", caller, &after, &before);
}

#[tokio::test]
async fn typescript_initializer_and_selected_constructor_assignment_both_evaluate() {
    let source = "const events: string[] = [];\nfunction value() { events.push('init'); return 'Paris'; }\nexport class Account {\n    city: string = value();\n    id: number;\n    constructor(id: number) {\n        this.id = id;\n        events.push(this.city);\n        this.city = 'Rome';\n    }\n    read(): string { return this.city; }\n}\nconst a = new Account(7);\nconsole.log(a.city, a.id, a.read(), events.join(','));\n";
    let ws = Workspace::new(&[("account.ts", source)]);
    let file = ws.path("account.ts");
    let refs = vec![
        at(&file, source, "push(this.city)", "city"),
        at(&file, source, "this.city = 'Rome'", "city"),
        at(&file, source, "return this.city", "city"),
        at(&file, source, "a.city, a.id", "city"),
    ];
    let gw = gateway(
        &file,
        source,
        "Account",
        "city",
        "city: string = value()",
        refs,
    )
    .await;
    run(
        &ws,
        "tsc",
        &["--target", "ES2022", "--module", "commonjs", "account.ts"],
    );
    let before = run(&ws, "node", &["account.js"]);
    extract(
        &ws,
        &gw,
        "account.ts",
        "Account",
        "city",
        &["read"],
        "Address",
        "address",
        true,
    )
    .await
    .unwrap();
    run(
        &ws,
        "tsc",
        &["--target", "ES2022", "--module", "commonjs", "account.ts"],
    );
    assert_eq!(run(&ws, "node", &["account.js"]), before);
    evidence(
        "initializer.ts",
        source,
        &fs::read_to_string(file).unwrap(),
        &before,
    );
}

#[tokio::test]
async fn selected_constructor_only_assignment_still_initializes_delegate() {
    let source = "class Account {\n    constructor(city) {\n        this.city = city;\n    }\n}\nconst a = new Account('Paris');\nconsole.log(a.city);\n";
    let ws = Workspace::new(&[("account.js", source)]);
    let file = ws.path("account.js");
    let gw = gateway(
        &file,
        source,
        "Account",
        "city",
        "this.city = city",
        vec![
            at(&file, source, "this.city = city", "city"),
            at(&file, source, "a.city);", "city"),
        ],
    )
    .await;
    let before = run(&ws, "node", &["account.js"]);
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
    assert_eq!(run(&ws, "node", &["account.js"]), before);
    evidence(
        "constructor.js",
        source,
        &fs::read_to_string(file).unwrap(),
        &before,
    );
}

#[tokio::test]
async fn unsupported_or_stale_identity_cannot_force_a_write() {
    for (index, expression) in [
        "a.city()",
        "a.city?.()",
        "(a.city)()",
        "a.city /* call */ ()",
        "a.missing",
    ]
    .iter()
    .enumerate()
    {
        let source = "class Account {\n    city = function() { return this; };\n}\nconst a = new Account();\n";
        let caller =
            format!("const other = {{ city: 3 }}; console.log({expression}, other.city);\n");
        let ws = Workspace::new(&[("account.js", source), ("caller.js", &caller)]);
        let token = if *expression == "a.missing" {
            "missing"
        } else {
            "city"
        };
        let gw = gateway(
            &ws.path("account.js"),
            source,
            "Account",
            "city",
            "city = function()",
            vec![at(&ws.path("caller.js"), &caller, expression, token)],
        )
        .await;
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
        let error = result.unwrap_err().to_string();
        assert!(
            error.contains("callable") || error.contains("does not identify"),
            "{error}"
        );
        assert_eq!(fs::read_to_string(ws.path("account.js")).unwrap(), source);
        assert_eq!(fs::read_to_string(ws.path("caller.js")).unwrap(), caller);
        evidence(&format!("refused-{index}.js"), &caller, &caller, &error);
    }
}

#[tokio::test]
async fn go_inaccessible_requested_names_are_explicit_and_atomic() {
    let owner = "package server\ntype Server struct {\n    Host string\n}\n";
    let caller = "package main\nimport srv \"example/server\"\nvar s = srv.Server{Host: \"Paris\"}\nfunc main() {}\n";
    for (helper, field) in [("endpoint", "Endpoint"), ("Endpoint", "endpoint")] {
        let ws = Workspace::new(&[
            ("go.mod", "module example\n\ngo 1.20\n"),
            ("server/server.go", owner),
            ("main.go", caller),
        ]);
        let gw = gateway(
            &ws.path("server/server.go"),
            owner,
            "Server",
            "Host",
            "Host string",
            vec![at(&ws.path("main.go"), caller, "Host: \"Paris\"", "Host")],
        )
        .await;
        let error = extract(
            &ws,
            &gw,
            "server/server.go",
            "Server",
            "Host",
            &[],
            helper,
            field,
            true,
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("exported requested name `endpoint`"),
            "{error}"
        );
        assert_eq!(
            fs::read_to_string(ws.path("server/server.go")).unwrap(),
            owner
        );
        assert_eq!(fs::read_to_string(ws.path("main.go")).unwrap(), caller);
    }
}

#[tokio::test]
async fn stale_same_name_literal_is_not_a_reference() {
    let source = "class Account {\n    city = 3;\n}\n";
    let caller = "console.log('a.city', { city: 9 }.city);\n";
    let ws = Workspace::new(&[("account.js", source), ("caller.js", caller)]);
    let gw = gateway(
        &ws.path("account.js"),
        source,
        "Account",
        "city",
        "city = 3",
        vec![at(&ws.path("caller.js"), caller, "a.city", "city")],
    )
    .await;
    let error = extract(
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
    .unwrap_err()
    .to_string();
    assert!(error.contains("literal/comment text"), "{error}");
    assert_eq!(fs::read_to_string(ws.path("account.js")).unwrap(), source);
    assert_eq!(fs::read_to_string(ws.path("caller.js")).unwrap(), caller);
    evidence("literal-refusal.js", caller, caller, &error);
}

#[tokio::test]
async fn typescript_wrapped_callable_refusals_preserve_receiver_runtime() {
    let owner = "export class Account {\n    city = function(this: Account) { return this.id; };\n    id = 7;\n}\n";
    for expression in [
        "(a.city as (() => number))()",
        "((a.city as /* type */\n ((strings: TemplateStringsArray) => number)))`x`",
        "(a.city as unknown as (() => number))()",
        "(a.city!)()",
        "(a.city)?. ()",
    ] {
        let caller = format!(
            "import {{ Account }} from './account';\nconst a = new Account(), other = {{ city: 3 }};\nconsole.log({expression}, other.city);\n"
        );
        let ws = Workspace::new(&[("account.ts", owner), ("main.ts", &caller)]);
        let gw = gateway(
            &ws.path("account.ts"),
            owner,
            "Account",
            "city",
            "city = function(this: Account)",
            vec![at(&ws.path("main.ts"), &caller, expression, "city")],
        )
        .await;
        run(
            &ws,
            "tsc",
            &["--target", "ES2022", "--module", "commonjs", "main.ts"],
        );
        let before = run(&ws, "node", &["main.js"]);
        assert_eq!(before, "7 3\n");
        let error = extract(
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
        .unwrap_err()
        .to_string();
        assert!(error.contains("callable field"), "{error}");
        assert_eq!(fs::read_to_string(ws.path("account.ts")).unwrap(), owner);
        assert_eq!(fs::read_to_string(ws.path("main.ts")).unwrap(), caller);
        assert_eq!(run(&ws, "node", &["main.js"]), before);
    }
}
