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

async fn check(name: &str, prefix: &str, rhs: &str, refusal: Option<&str>) {
    let source = format!(
        "const events: string[] = [];\nclass Account {{\n    readonly city: string | undefined;\n    constructor(city: string | undefined) {{\n{prefix}        this.city = {rhs};\n    }}\n}}\nconst a = new Account(undefined);\nconst b = new Account('Paris');\nconsole.log(a.city, b.city, events.join(','));\n"
    );
    let ws = Workspace::new(&[("account.ts", &source)]);
    let file = ws.path("account.ts");
    let mut refs = ["this.city =", "a.city,", "b.city,"]
        .iter()
        .map(|anchor| at(&file, &source, anchor, "city"))
        .collect::<Vec<_>>();
    if rhs.contains("this.city") {
        refs.push(at(&file, &source, rhs, "city"));
    }
    let gw = gateway(&file, &source, "Account", "city", "readonly city:", refs).await;
    let compile = || {
        run(
            &ws,
            "tsc",
            &["--target", "ES2022", "--module", "commonjs", "account.ts"],
        )
    };
    compile();
    let before = run(&ws, "node", &["account.js"]);
    println!("{name} original Node stdout: {before:?}");
    assert_eq!(
        before,
        if prefix.contains("events.push") {
            "undefined Paris before:Paris\n"
        } else {
            "undefined Paris \n"
        }
    );
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
    match refusal {
        Some(message) => {
            let error = result.unwrap_err();
            assert!(error.to_string().contains(message), "{error:#}");
            assert_eq!(fs::read_to_string(&file).unwrap(), source);
        }
        None => {
            result.unwrap();
            let after = fs::read_to_string(&file).unwrap();
            println!("{name} extracted source:\n{after}");
            assert!(after.contains("readonly city: string | undefined;"));
            assert!(after.contains("this.address = new Address(city);"));
        }
    }
    compile();
    let after_output = run(&ws, "node", &["account.js"]);
    assert_eq!(after_output, before);
    evidence(
        name,
        &source,
        &fs::read_to_string(file).unwrap(),
        &after_output,
    );
}

#[tokio::test]
async fn readonly_rhs_self_read_refuses_before_writes() {
    check(
        "readonly-self-read.ts",
        "",
        "this.city ?? city",
        Some("deferred initializer has unproved owner-dependent evaluation"),
    )
    .await;
}

#[tokio::test]
async fn readonly_early_return_refuses_before_writes() {
    check(
        "readonly-early-return.ts",
        "        if (city === undefined) return;\n",
        "city",
        Some("readonly initialization has unproved earlier owner/control-flow effects"),
    )
    .await;
    check(
        "readonly-block-return.ts",
        "        if (city === undefined) {\n            return;\n        }\n",
        "city",
        Some("readonly initialization has unproved earlier owner/control-flow effects"),
    )
    .await;
}

#[tokio::test]
async fn readonly_conditional_assignment_refuses_before_writes() {
    check(
        "readonly-conditional.ts",
        "        if (city !== undefined)\n",
        "city",
        Some("readonly initialization has unproved earlier owner/control-flow effects"),
    )
    .await;
}

#[tokio::test]
async fn readonly_nested_block_keeps_constructor_boundary() {
    check(
        "readonly-nested-block.ts",
        "        if (city) {\n            events.push('before:' + city);\n        }\n",
        "city",
        None,
    )
    .await;
}
