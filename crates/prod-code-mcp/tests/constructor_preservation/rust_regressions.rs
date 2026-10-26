/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::support::{bytes, check, record, run, transform};
use prod_code_mcp::replace_constructor::ReplaceMode::{self, Builder, Factory};
use prod_code_testkit::Workspace;
use serde_json::json;

const STANDALONE: &str = "struct Config { value: i32 }\nfn main() { let c = Config { value: 7 }; println!(\"{}\", c.value); }\n";

async fn standalone(mode: ReplaceMode) {
    let ws = Workspace::new(&[("main.rs", STANDALONE)]);
    let root = ws.root();
    let before_compile = run(
        &root,
        &["rustc", "--edition=2021", "main.rs", "-o", "program"],
    );
    let before = run(&root, &["./program"]);
    let preview = transform(&ws, "rs", "Config", mode, false).await;
    let apply = transform(&ws, "rs", "Config", mode, true).await;
    let output = std::fs::read_to_string(root.join("main.rs")).unwrap();
    let after_compile = run(
        &root,
        &["rustc", "--edition=2021", "main.rs", "-o", "program"],
    );
    let after = run(&root, &["./program"]);
    let at = STANDALONE.rfind("Config").unwrap();
    record(
        &format!("standalone-{mode}"),
        &json!({
            "input":bytes(&root, STANDALONE), "output":bytes(&root, &output),
            "reference_fixture":{"file":"main.rs","line":1,"character":at - STANDALONE.find('\n').unwrap() - 1,"length":6},
            "scripted_transport":"exact reference, diagnostic sentinel and force=true; not live analyzer certification",
            "before_compile":before_compile,"before":before,"after_compile":after_compile,"after":after,
            "preview":preview.as_ref().map(|r| json!({"count":r.instantiations_rewritten,"blocked":r.blocked})).unwrap_or_else(|e| json!({"error":format!("{e:#}").replace(&*root.to_string_lossy(), "<fixture>")})),
            "apply":apply.as_ref().map(|r| json!({"count":r.instantiations_rewritten,"blocked":r.blocked})).unwrap_or_else(|e| json!({"error":format!("{e:#}").replace(&*root.to_string_lossy(), "<fixture>")}))
        }),
    );
    assert_eq!(before_compile["exit"], 0, "{before_compile}");
    assert_eq!(before["exit"], 0, "{before}");
    assert_eq!(before["stdout"], "7\n");
    assert_eq!(preview.unwrap().instantiations_rewritten, 1);
    assert_eq!(apply.unwrap().instantiations_rewritten, 1);
    assert_eq!(after_compile["exit"], 0, "{after_compile}");
    assert_eq!(after["exit"], 0, "{after}");
    assert_eq!(before["stdout"], after["stdout"]);
}

#[tokio::test]
async fn standalone_factory() {
    standalone(Factory).await;
}

#[tokio::test]
async fn standalone_builder() {
    standalone(Builder).await;
}

const ALIASES: &str = r#"struct Config { value: i32 }
mod alias {
    type Config = crate::Config;
    impl Config { pub fn make() -> Self { Self { value: 7 } } }
}
mod chained {
    type Intermediate = crate::Config;
    type Config = Intermediate;
    impl Config { pub fn chained() -> Self { Self { value: 8 } } }
}
mod unrelated {
    pub struct Config { pub value: i32 }
    type Alias = Config;
    impl Alias {
        pub fn new(value: i32) -> Self { let mut c = Self { value: 0 }; c.value = value + 10; c }
        pub fn make() -> Self { Self { value: 9 } }
    }
}
fn main() {
    let unrelated = unrelated::Config::make().value;
    let a = Config::make().value;
    let b = Config::chained().value;
    println!("{} {} {}", a, b, unrelated);
}
"#;

async fn alias_owner(mode: ReplaceMode) {
    check(
        &format!("alias-owner-{mode}"),
        "rs",
        "Config",
        ALIASES,
        mode,
        2,
        &["Self { value: 9 }", "Self { value: 0 }", "value + 10"],
    )
    .await;
}

#[tokio::test]
async fn alias_owner_factory() {
    alias_owner(Factory).await;
}

#[tokio::test]
async fn alias_owner_builder() {
    alias_owner(Builder).await;
}

#[tokio::test]
async fn malformed_manifest_remains_an_error() {
    let ws = Workspace::new(&[("main.rs", STANDALONE), ("Cargo.toml", "[package\n")]);
    for mode in [Factory, Builder] {
        let result = transform(&ws, "rs", "Config", mode, true).await;
        let output = std::fs::read_to_string(ws.root().join("main.rs")).unwrap();
        record(
            &format!("malformed-manifest-{mode}"),
            &json!({"input":bytes(&ws.root(), STANDALONE),"output":bytes(&ws.root(), &output),
                "error":result.as_ref().err().map(|e| format!("{e:#}").replace(&*ws.root().to_string_lossy(), "<fixture>"))}),
        );
        assert!(result.unwrap_err().to_string().contains("Cargo.toml"));
        assert_eq!(output, STANDALONE);
    }
}
