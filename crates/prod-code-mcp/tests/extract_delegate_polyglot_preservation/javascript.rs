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
async fn exact_identity_and_declaring_file_uses() {
    let source = "function before(a) { return a.city; }\nclass Account {\n    city = 'Paris';\n    read() { return this.city; }\n}\nclass Other { city = 'Rome'; }\nconst a = new Account(), other = new Other();\nconsole.log(before(a), a.city, other.city, '.city');\n";
    let ws = Workspace::new(&[("account.js", source)]);
    let file = ws.path("account.js");
    let refs = vec![
        at(&file, source, "a.city;", "city"),
        at(&file, source, "this.city;", "city"),
        at(&file, source, "a.city, other", "city"),
    ];
    let gw = gateway(&file, source, "Account", "city", "city = 'Paris'", refs).await;
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
    let after = fs::read_to_string(&file).unwrap();
    assert!(
        after.contains("a.address.city, other.city, '.city'"),
        "{after}"
    );
    assert!(after.contains("this.address.city"), "{after}");
    assert_eq!(run(&ws, "node", &["account.js"]), before);
    evidence("identity.js", source, &after, &before);
}

#[tokio::test]
async fn unrelated_constructor_assignment_preserves_initializer() {
    let source = "const events = [];\nfunction value() { events.push('city'); return 'Paris'; }\nclass Account {\n    city = value();\n    id;\n    constructor(id) {\n        this.id = id;\n    }\n}\nconst a = new Account(7);\nconsole.log(a.city, a.id, events.join(','));\n";
    let ws = Workspace::new(&[("account.js", source)]);
    let file = ws.path("account.js");
    let gw = gateway(
        &file,
        source,
        "Account",
        "city",
        "city = value()",
        vec![at(&file, source, "a.city, a.id", "city")],
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
    let after = fs::read_to_string(file).unwrap();
    assert_eq!(run(&ws, "node", &["account.js"]), before);
    evidence("initializer.js", source, &after, &before);
}

#[tokio::test]
async fn callable_identity_refuses_without_rewriting_unrelated_property() {
    let source = "class Account {\n    city = function() { return this.id; };\n    id = 7;\n}\nmodule.exports = { Account };\n";
    let caller = "const { Account } = require('./account');\nconst a = new Account();\nconst other = { city: 3 }; console.log(a.city(), other.city);\n";
    let ws = Workspace::new(&[("account.js", source), ("caller.js", caller)]);
    let file = ws.path("account.js");
    let gw = gateway(
        &file,
        source,
        "Account",
        "city",
        "city = function()",
        vec![at(&ws.path("caller.js"), caller, "a.city()", "city")],
    )
    .await;
    let before = run(&ws, "node", &["caller.js"]);
    assert_eq!(before, "7 3\n");
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
    assert!(
        result.is_err(),
        "callable field receiver must be preserved or refused"
    );
    assert_eq!(fs::read_to_string(file).unwrap(), source);
    assert_eq!(fs::read_to_string(ws.path("caller.js")).unwrap(), caller);
    assert_eq!(run(&ws, "node", &["caller.js"]), before);
    evidence("callable-refusal.js", caller, caller, &before);
}

#[tokio::test]
async fn final_review_comparison_and_shift_read_remains_supported() {
    let source = "class Account {\n    city = 1;\n}\nconst a = new Account();\nconsole.log(a.city < 10 < 20 >> (1));\n";
    let ws = Workspace::new(&[("account.js", source)]);
    let file = ws.path("account.js");
    let gw = gateway(
        &file,
        source,
        "Account",
        "city",
        "city = 1",
        vec![at(&file, source, "a.city <", "city")],
    )
    .await;
    println!("CASE javascript-comparison\nBEFORE SOURCE\n{source}END SOURCE");
    let before = run(&ws, "node", &["account.js"]);
    println!("node before exit 0: {before}");
    assert_eq!(before, "true\n");
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
    let after = fs::read_to_string(file).unwrap();
    println!("AFTER SOURCE\n{after}END SOURCE");
    result.unwrap();
    assert!(after.contains("a.address.city < 10 < 20 >> (1)"));
    let output = run(&ws, "node", &["account.js"]);
    println!("node after exit 0: {output}");
    assert_eq!(output, before);
}
