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
use prod_code_mcp::{move_item::move_item, move_module::move_module};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};

const MANIFEST: &str =
    "[package]\nname='p'\nversion='0.1.0'\nedition='2021'\n[features]\nenabled=[]\n";

pub(super) async fn attempt(
    ws: &Workspace,
    whole: bool,
    source: &str,
    target: &str,
    name: &str,
    refs: Vec<(&str, u32, u32)>,
    force: bool,
) -> Value {
    let name = name.to_owned();
    let mut locations = Vec::new();
    for (file, line, col) in refs {
        locations.extend(
            answers::locations(&ws.path(file), &[(line, col)])
                .as_array()
                .unwrap()
                .clone(),
        );
    }
    let gateway = ScriptedGateway::start(move |method, _| match method {
        "textDocument/documentSymbol" => json!([answers::document_symbol(&name, 12, 1, 1, 1)]),
        "textDocument/references" => json!(locations),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await;
    let before = snapshot(&ws.root());
    let returned = if whole {
        match move_module(
            gateway.addr(),
            &ws.root(),
            &ws.path(source),
            &ws.path(target),
        )
        .await
        {
            Ok(mut plan) => match plan.write(true) {
                Ok(()) => json!({"applied": plan.applied}),
                Err(e) => json!({"error": format!("{e:#}")}),
            },
            Err(e) => json!({"error": format!("{e:#}")}),
        }
    } else {
        match move_item(
            gateway.addr(),
            &ws.root(),
            &ws.path(source),
            1,
            1,
            &ws.path(target),
            true,
            force,
        )
        .await
        {
            Ok(plan) => json!({"applied": plan.applied}),
            Err(e) => json!({"error": format!("{e:#}")}),
        }
    };
    json!({"before": before, "returned": returned, "after": snapshot(&ws.root()),
        "analyzer": "scripted exact references and clean diagnostics; compiler and runtime evidence recorded separately"})
}

#[tokio::test]
async fn review_cfg_membership_preserves_public_api_or_refuses_atomically() {
    let mut failures = Vec::new();
    for whole in [false, true] {
        for force in [false, true] {
            for (cfg, active) in [("any()", false), ("all()", true), ("not(any())", true)] {
                let lib = format!("pub mod source;\n#[cfg({cfg})] pub mod dest;\n");
                let ws = Workspace::new(&[
                    ("Cargo.toml", MANIFEST),
                    ("src/lib.rs", &lib),
                    ("src/main.rs", "mod source;\nmod dest;\nfn main() {}\n"),
                    ("src/source.rs", "pub struct Api;\n"),
                    ("src/dest.rs", ""),
                    (
                        "tests/api.rs",
                        "#[test] fn api() { let _ = p::source::Api; }\n",
                    ),
                ]);
                let before = cargo(&ws.root());
                let target = if whole {
                    "src/dest/source.rs"
                } else {
                    "src/dest.rs"
                };
                let mut record =
                    attempt(&ws, whole, "src/source.rs", target, "Api", vec![], force).await;
                if record["returned"]["applied"] == true {
                    let api = if whole { "dest::source" } else { "dest" };
                    ws.write(
                        "tests/api.rs",
                        &format!("#[test] fn api() {{ let _ = p::{api}::Api; }}\n"),
                    );
                }
                record["before_compiler"] = before;
                record["after_compiler"] = cargo(&ws.root());
                save_named(
                    &format!("ownership-membership-{whole}-{force}-{cfg}"),
                    record.clone(),
                );
                let behavior = if active {
                    record["returned"]["applied"] == true
                } else {
                    record["returned"].get("error").is_some() && record["before"] == record["after"]
                };
                if !(behavior
                    && record["before_compiler"]["passed"] == true
                    && record["after_compiler"]["passed"] == true)
                {
                    failures.push(record);
                }
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[tokio::test]
async fn review_same_named_external_binding_and_shared_caller_runtime() {
    let mut failures = Vec::new();
    for whole in [false, true] {
        for shared in [false, true] {
            for decoy in [false, true] {
                let lib = if shared {
                    "extern crate self as p;\npub mod a;\npub mod c;\npub mod caller;\n"
                } else {
                    "pub mod a;\npub mod c;\n"
                };
                let inline = if decoy {
                    "mod c { pub fn value() -> u8 { 9 } pub mod b { pub fn value() -> u8 { 9 } } }\n"
                } else {
                    ""
                };
                let bin = if shared {
                    format!(
                        "{inline}mod caller;\nfn main() {{ caller::call(); }}\n#[test] fn runtime() {{ main(); }}\n"
                    )
                } else {
                    format!(
                        "{inline}fn main() {{ assert_eq!(p::a::b::value(), 7); }}\n#[test] fn runtime() {{ main(); }}\n"
                    )
                };
                let ws = Workspace::new(&[
                    ("Cargo.toml", MANIFEST),
                    ("src/lib.rs", lib),
                    ("src/main.rs", &bin),
                    ("src/a.rs", "pub mod b;\n"),
                    ("src/a/b.rs", "pub fn value() -> u8 { 7 }\n"),
                    ("src/c.rs", ""),
                    (
                        "src/caller.rs",
                        "pub fn call() { assert_eq!(p::a::b::value(), 7); }\n",
                    ),
                ]);
                let before = cargo(&ws.root());
                let caller = if shared {
                    "src/caller.rs"
                } else {
                    "src/main.rs"
                };
                let line = if shared || !decoy { 1 } else { 2 };
                let text = ws.read(caller);
                let needle = if whole { "b::value" } else { "value()" };
                let col = text.lines().nth(line - 1).unwrap().find(needle).unwrap() as u32 + 1;
                let target = if whole { "src/c/b.rs" } else { "src/c.rs" };
                let mut record = attempt(
                    &ws,
                    whole,
                    "src/a/b.rs",
                    target,
                    "value",
                    vec![(caller, line as u32, col)],
                    true,
                )
                .await;
                record["before_compiler"] = before;
                record["after_compiler"] = cargo(&ws.root());
                save_named(
                    &format!("ownership-external-{whole}-{shared}-{decoy}"),
                    record.clone(),
                );
                let behavior = if shared {
                    (record["returned"].get("error").is_some()
                        && record["before"] == record["after"])
                        || record["returned"]["applied"] == true
                } else {
                    record["returned"]["applied"] == true
                };
                if !(behavior
                    && record["before_compiler"]["passed"] == true
                    && record["after_compiler"]["passed"] == true)
                {
                    failures.push(record);
                }
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[tokio::test]
async fn review_cfg_attr_literal_and_uncertainty_are_local() {
    let mut failures = Vec::new();
    for whole in [false, true] {
        for (index, attribute) in [
            "#[cfg_attr(all(), path=\"odd.rs\")]",
            "#[cfg_attr(any(), path=\"missing.rs\")]",
            "#[cfg_attr(all(), cfg_attr(not(any()), path=r#\"odd.rs\"#))]",
            "#[cfg_attr(feature=\"enabled\", path=\"odd.rs\")]",
        ]
        .into_iter()
        .enumerate()
        {
            let lib = format!("pub mod a;\npub mod c;\n{attribute} pub mod unrelated;\n");
            let ws = Workspace::new(&[
                ("Cargo.toml", MANIFEST),
                ("src/lib.rs", &lib),
                ("src/a.rs", "pub fn f() -> u8 { 7 }\n"),
                ("src/c.rs", ""),
                ("src/odd.rs", "pub struct Odd;\n"),
                ("src/unrelated.rs", "pub struct Other;\n"),
                (
                    "tests/api.rs",
                    "#[test] fn api() { assert_eq!(p::a::f(), 7); }\n",
                ),
            ]);
            let before = cargo(&ws.root());
            let target = if whole { "src/c/a.rs" } else { "src/c.rs" };
            let mut record = attempt(&ws, whole, "src/a.rs", target, "f", vec![], true).await;
            if record["returned"]["applied"] == true {
                let api = if whole { "c::a" } else { "c" };
                ws.write(
                    "tests/api.rs",
                    &format!("#[test] fn api() {{ assert_eq!(p::{api}::f(), 7); }}\n"),
                );
            }
            record["before_compiler"] = before;
            record["after_compiler"] = cargo(&ws.root());
            save_named(
                &format!("ownership-unrelated-{whole}-{index}"),
                record.clone(),
            );
            if !(record["before_compiler"]["passed"] == true
                && record["returned"]["applied"] == true
                && record["after_compiler"]["passed"] == true)
            {
                failures.push(record);
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
