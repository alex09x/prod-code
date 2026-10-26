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

#[tokio::test]
async fn repair_review_comparison_defaults_refuse() {
    for parameters in [
        "value = 1 < 2",
        "value = 2 > 1",
        "value = 1 < 2, other = 3 > 2",
        "value: Array<string> = []",
        "value = (1 < 2 ? 1 : 2)",
    ] {
        declaration_call(&format!("function completed({parameters}) {{}}")).await;
    }
}

#[tokio::test]
async fn repair_review_postfix_asi_refuses() {
    for operator in ["++", "--"] {
        declaration_call(&format!(
            "let count = 0; count{operator}\nfunction completed() {{}}"
        ))
        .await;
    }
}

async fn initializer(value: &str, refusal: Option<&str>) {
    let source = format!(
        "class Account {{\n    readonly city: string;\n    label = {value};\n    constructor(city: string) {{\n        this.city = city;\n    }}\n    current(): string | undefined {{ return this.city; }}\n}}\nconst a = new Account('Paris');\nconsole.log(a.label, a.city);\n"
    );
    check(
        value,
        &source,
        &["this.city =", "return this.city", "a.city)"],
        "undefined Paris\n",
        refusal,
    )
    .await;
}

#[tokio::test]
async fn repair_review_template_regex_refuses() {
    for value in [
        "`${/}/.test('') ? this.current() : this.current()}`",
        "`${/[}]/.test('') ? this.current() : this.current()}`",
        "`${/\\}/.test('') ? this.current() : this.current()}`",
        "`${1 / 2 ? this.current() : this.current()}`",
    ] {
        initializer(
            value,
            Some("readonly initialization has unproved owner field-initializer effects"),
        )
        .await;
    }
}

#[tokio::test]
async fn repair_review_ambiguous_regex_backtick_refuses() {
    initializer(
        "`${/`}/.test('') ? this.current() : this.current()}`",
        Some("class has unproved template/regex lexical boundaries"),
    )
    .await;
}

#[tokio::test]
async fn repair_review_typed_super_methods_remain_supported() {
    for value in [
        "({ super(): number { return 1; } }).super()",
        "({ super()\n{ return 1; } }).super()",
        "({ super(): number\n{ return 1; } }).super()",
        "({ super(): { value: number } { return { value: 1 }; } }).super().value",
    ] {
        let source = format!(
            "class Account {{\n    readonly city: string;\n    metadata = {value};\n    constructor(city: string) {{\n        this.city = city;\n    }}\n}}\nconsole.log(new Account('Paris').city);\n"
        );
        check(value, &source, &["this.city =", ").city)"], "Paris\n", None).await;
    }
}

#[tokio::test]
async fn repair_review_comparison_expression_arguments_remain_supported() {
    for parameters in ["value: string, flag = 1 < 2", "value: string, flag = 2 > 1"] {
        let source = format!(
            "class Account {{\n    city = 'Paris';\n}}\nconst a = new Account();\nconst read = function({parameters}) {{ return () => value; }}(a.city)();\nconsole.log(read);\n"
        );
        check(parameters, &source, &["a.city"], "Paris\n", None).await;
    }
}

#[tokio::test]
async fn repair_review_regex_and_division_data_remain_supported() {
    for value in ["`${/}/.test('')}`", "`${/[}]/.test('')}`", "`${1 / 2}`"] {
        let source = format!(
            "class Account {{\n    readonly city: string;\n    label = {value};\n    constructor(city: string) {{\n        this.city = city;\n    }}\n}}\nconsole.log(new Account('Paris').city);\n"
        );
        check(value, &source, &["this.city =", ").city)"], "Paris\n", None).await;
    }
}

#[tokio::test]
async fn remaining_review_typescript_declaration_call_refuses() {
    for declaration in [
        "interface Completed {}",
        "interface Completed extends Array<string> {}",
        "type Completed = {}\n",
        "enum Completed { Value }",
        "const enum Completed { Value }",
        "namespace Completed {}",
        "declare class Completed {}",
    ] {
        declaration_call(declaration).await;
    }
}

#[tokio::test]
async fn remaining_review_member_keyword_control_call_refuses() {
    for member in ["function", "class"] {
        declaration_call(&format!(
            "const guard = {{ {member}() {{ return true; }} }}; if (guard.{member}()) {{}}",
        ))
        .await;
    }
}

#[tokio::test]
async fn remaining_review_property_keyword_division_refuses() {
    let source = "class Account {\n    readonly city: number;\n    metadata = ({ in: 2 }).in / this.current();\n    constructor(city: number) {\n        this.city = city;\n    }\n    current(): number { return this.city; }\n}\nconst a = new Account(7);\nconsole.log(a.metadata, a.city);\n";
    check(
        "property keyword division owner effect",
        source,
        &["this.city =", "return this.city", "a.city)"],
        "NaN 7\n",
        Some("readonly initialization has unproved owner field-initializer effects"),
    )
    .await;
}

#[tokio::test]
async fn remaining_review_property_keyword_data_remains_supported() {
    for value in [
        "({ in: 2 }).in / 2",
        "({ in: 2 }). /* property */ in / 2",
        "({ in: 2 })?.in / 2",
        "`${({ in: 2 }).in / 2}`",
        "({ in: 2 }).in / `${'this.current()'}`.length",
        "'this' in /this.current()/",
    ] {
        let source = format!(
            "class Account {{\n    readonly city: string;\n    metadata = {value};\n    constructor(city: string) {{\n        this.city = city;\n    }}\n}}\nconsole.log(new Account('Paris').city);\n"
        );
        check(value, &source, &["this.city =", ").city)"], "Paris\n", None).await;
    }
}

#[tokio::test]
async fn remaining_review_member_keyword_arguments_remain_supported() {
    for member in ["function", "class"] {
        let source = format!(
            "class Account {{\n    city = 'Paris';\n}}\nconst a = new Account();\nconst factory = {{ {member}(value: string) {{ return () => value; }} }};\nconsole.log(factory.{member}(a.city)());\n"
        );
        check(member, &source, &["a.city"], "Paris\n", None).await;
    }
}
