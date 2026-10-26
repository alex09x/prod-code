/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::remaining_findings::{check, declaration_call};

async fn division(value: &str, refusal: bool) {
    let source = format!(
        "const π = 2;\nconst value: number | undefined = 2;\nclass Account {{\n    readonly city: number;\n    metadata = {value};\n    constructor(city: number) {{\n        this.city = city;\n    }}\n    current(): number {{ return this.city; }}\n}}\nconst a = new Account(7);\nconsole.log(a.metadata, a.city);\n"
    );
    check(
        value,
        &source,
        &["this.city =", "return this.city", "a.city)"],
        if refusal { "NaN 7\n" } else { "1 7\n" },
        refusal.then_some("readonly initialization has unproved owner field-initializer effects"),
    )
    .await;
}

#[tokio::test]
async fn lexical_bypasses_unicode_division_refuses() {
    division("π / this.current()", true).await;
}

#[tokio::test]
async fn lexical_bypasses_non_null_division_refuses() {
    division("value! / this.current()", true).await;
}

#[tokio::test]
async fn lexical_bypasses_regex_asi_declaration_refuses() {
    for pattern in ["/x/", "/[}]/", "/x/g"] {
        declaration_call(&format!(
            "const pattern = {pattern}\nfunction completed() {{}}"
        ))
        .await;
    }
}

async fn base_construction(prefix: &str) {
    let source = format!(
        "class Base {{\n    constructor() {{ console.log(this.current()); }}\n    current(): string | undefined {{ return 'base'; }}\n}}\nclass Account extends Base {{\n    readonly city: string;\n    constructor(city: string) {{\n        {prefix}\n        this.city = city;\n    }}\n    current(): string | undefined {{ return this.city; }}\n}}\nconsole.log(new Account('Paris').city);\n"
    );
    check(
        prefix,
        &source,
        &["this.city =", "return this.city", ").city)"],
        "undefined\nParis\n",
        Some("readonly initialization has unproved earlier owner/control-flow effects"),
    )
    .await;
}

#[tokio::test]
async fn lexical_bypasses_labeled_super_refuses() {
    base_construction("label: { super()\n        {} }").await;
}

#[tokio::test]
async fn lexical_bypasses_bare_block_super_refuses() {
    base_construction("{ super()\n        {} }").await;
}

#[tokio::test]
async fn lexical_bypasses_division_data_remains_supported() {
    for value in ["π / 2", "value! / 2", "value!! / 2"] {
        division(value, false).await;
    }
}

#[tokio::test]
async fn lexical_bypasses_super_method_data_remains_supported() {
    for value in [
        "{ super(): number { return 1; } }",
        "{ nested: { super(): number { return 1; } } }",
        "false ? {} : { super(): number { return 1; } }",
        "! /this.current()/.test('')",
    ] {
        let source = format!(
            "class Account {{\n    readonly city: string;\n    metadata = {value};\n    constructor(city: string) {{\n        this.city = city;\n    }}\n}}\nconsole.log(new Account('Paris').city);\n"
        );
        check(value, &source, &["this.city =", ").city)"], "Paris\n", None).await;
    }
}

#[tokio::test]
async fn lexical_bypasses_constructor_object_data_remains_supported() {
    let source = "class Account {\n    readonly city: string;\n    constructor(city: string) {\n        const metadata = false ? {} : { super(): void {} };\n        this.city = city;\n    }\n}\nconsole.log(new Account('Paris').city);\n";
    check(
        "constructor object data",
        source,
        &["this.city =", ").city)"],
        "Paris\n",
        None,
    )
    .await;
}

#[tokio::test]
async fn lexical_bypasses_regex_function_arguments_remain_supported() {
    let source = "class Account {\n    city = 'Paris';\n}\nconst a = new Account();\nconst pattern = /x/;\nconst read = function(value: string) { return () => value; }(a.city)();\nconsole.log(read);\n";
    check(
        "regex before function expression",
        source,
        &["a.city"],
        "Paris\n",
        None,
    )
    .await;
}
