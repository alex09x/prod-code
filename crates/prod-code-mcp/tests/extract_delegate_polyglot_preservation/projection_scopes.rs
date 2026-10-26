/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The JS/TS delegate projection reads the selected class from its parsed syntax (#1409):
//! every instance initializer however it is laid out, the export modifier only where the
//! program has one, and a receiver as the receiver of the function that owns it. Each case
//! is typechecked with tsc and run with node through `check`.

use super::remaining_findings::check;

const OWNER_EFFECTS: &str = "readonly initialization has unproved owner field-initializer effects";

#[tokio::test]
async fn an_initializer_continued_on_the_next_line_is_guarded() {
    for (label, initializer) in [
        ("semicolon", "metadata =\n        of / this.current();"),
        ("asi", "metadata =\n        of / this.current()"),
    ] {
        let source = format!(
            "const of = 14;\nclass Account {{\n    readonly city: number;\n    {initializer}\n    constructor(city: number) {{\n        this.city = city;\n    }}\n    current() {{ return this.city; }}\n}}\nconst account = new Account(7);\nconsole.log(Number.isNaN(account.metadata), account.current());\n"
        );
        check(
            label,
            &source,
            &["this.city =", "return this.city"],
            "true 7\n",
            Some(OWNER_EFFECTS),
        )
        .await;
    }
}

#[tokio::test]
async fn a_commented_export_is_not_an_export_modifier() {
    for comment in ["// export\n", "/* export */ ", "/* abstract */ "] {
        let source = format!(
            "{comment}class Account {{\n    city = 'Paris';\n}}\nconst a = new Account();\nconsole.log(a.city);\n"
        );
        check(comment, &source, &["a.city"], "Paris\n", None).await;
    }
}

#[tokio::test]
async fn a_receiver_owned_by_a_nested_method_is_not_the_owners() {
    let source = "class Account {\n    readonly city: string;\n    metadata = { value: 1, read() { return this.value; } };\n    constructor(city: string) {\n        this.city = city;\n    }\n}\nconst a = new Account('Paris');\nconsole.log(a.city, a.metadata.read());\n";
    check(
        "nested-method",
        source,
        &["this.city =", "a.city,"],
        "Paris 1\n",
        None,
    )
    .await;

    // An arrow function has no receiver of its own: `this` is still the owner.
    let source = "class Account {\n    readonly city: string;\n    metadata = { read: () => this.current() };\n    constructor(city: string) {\n        this.city = city;\n    }\n    current(): string | undefined { return this.city; }\n}\nconst a = new Account('Paris');\nconsole.log(a.city, a.metadata.read());\n";
    check(
        "nested-arrow",
        source,
        &["this.city =", "return this.city", "a.city,"],
        "Paris Paris\n",
        Some(OWNER_EFFECTS),
    )
    .await;
}
