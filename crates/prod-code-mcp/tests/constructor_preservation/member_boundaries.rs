/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::aggregate_repair_support::{Case, check};

async fn member(id: &str, source: &str, preserved: &[&str]) {
    check(Case {
        id,
        files: &[("main.mjs", source)],
        selected: "main.mjs",
        target: "Bag",
        count: source.matches("new Bag").count(),
        refuse: false,
        blocked_preview: false,
        references: source
            .match_indices("new Bag")
            .map(|(at, _)| ("main.mjs", at + 4, 3))
            .collect(),
        preserved,
    })
    .await;
}

#[tokio::test]
async fn keyword_member_static_property() {
    member(
        "member-asi-static-property",
        "const settings={static:0}; class Bag { label=settings.static\nconstructor(value,extra){this.saved=value+extra;} } console.log(new Bag(7,2).saved);\n",
        &["label=settings.static", "constructor(value,extra)"],
    )
    .await;
}

#[tokio::test]
async fn keyword_member_property_names() {
    for keyword in ["function", "new", "get", "set", "async"] {
        let source = format!(
            "const settings={{{keyword}:0}}; class Bag {{\n label=settings?. /* property */ {keyword} /* complete */\n constructor(value=7,...rest){{this.saved=value+rest.reduce((a,b)=>a+b,0);}}\n}} console.log(new Bag().saved,new Bag(7,2,3).saved);\n"
        );
        member(
            &format!("member-asi-{keyword}-property"),
            &source,
            &["constructor(value=7,...rest)", "label=settings?."],
        )
        .await;
    }
}

#[tokio::test]
async fn keyword_member_private_property_names() {
    for keyword in ["static", "function", "new"] {
        let source = format!(
            "class Bag {{\n #{keyword}=0; label=this.#{keyword}\n constructor(value=7,...rest){{this.saved=value+rest.reduce((a,b)=>a+b,0);}}\n}} console.log(new Bag().saved,new Bag(7,2,3).saved);\n"
        );
        member(
            &format!("member-asi-private-{keyword}-property"),
            &source,
            &[
                "constructor(value=7,...rest)",
                &format!("label=this.#{keyword}"),
            ],
        )
        .await;
    }
}

#[tokio::test]
async fn keyword_member_expression_and_modifier_contexts() {
    for (id, field) in [
        (
            "function",
            "label = function\n constructor(unrelated) { return unrelated; }",
        ),
        (
            "generator",
            "label = function /* name follows star */ *\n constructor(unrelated) { yield unrelated; }",
        ),
        (
            "async-generator",
            "label = async function*\n constructor(unrelated) { yield unrelated; }",
        ),
        ("new", "label = new\n constructor(0)"),
        (
            "static",
            "label = 0\n static\n constructor(unrelated) { return unrelated; }",
        ),
    ] {
        let source = format!(
            "function constructor(value){{this.value=value;}}\nclass Bag {{\n {field}\n constructor(value=7,...rest){{this.saved=value+rest.reduce((a,b)=>a+b,0);}}\n}} console.log(new Bag().saved,new Bag(7,2,3).saved);\n"
        );
        member(
            &format!("member-context-{id}"),
            &source,
            &[field, "constructor(value=7,...rest)"],
        )
        .await;
    }
}
