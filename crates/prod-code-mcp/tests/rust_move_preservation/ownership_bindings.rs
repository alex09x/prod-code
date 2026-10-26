/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{ownership_review::attempt, support::*};
use prod_code_testkit::Workspace;
use serde_json::json;

#[tokio::test]
async fn custom_library_and_nested_extern_bindings_preserve_runtime_identity() {
    let mut failures = Vec::new();
    for whole in [false, true] {
        for edition in ["2015", "2021"] {
            for renamed in [false, true] {
                let dependency = if renamed {
                    "renamed={path='..',package='p'}"
                } else {
                    "p={path='..'}"
                };
                let external = if renamed { "renamed" } else { "api" };
                let binding = if edition == "2015" {
                    "binding"
                } else {
                    external
                };
                let declaration = if edition == "2015" {
                    format!("extern crate {external} as binding;\n")
                } else {
                    String::new()
                };
                let original = format!("::{binding}::a::b::value()");
                let bin = format!(
                    "{declaration}mod caller {{\nmod {binding} {{ pub mod c {{ pub fn value() -> u8 {{ 9 }} pub mod b {{ pub fn value() -> u8 {{ 9 }} }} }} }}\npub fn run() {{ assert_eq!({original}, 7); }}\n}}\nfn main() {{ caller::run(); }}\n#[test] fn runtime() {{ main(); }}\n"
                );
                let manifest = format!(
                    "[package]\nname='runner'\nversion='0.1.0'\nedition='{edition}'\n[dependencies]\n{dependency}\n[[bin]]\nname='p'\npath='src/main.rs'\n"
                );
                let ws = Workspace::new(&[
                    (
                        "Cargo.toml",
                        "[workspace]\nmembers=['runner']\n[package]\nname='p'\nversion='0.1.0'\nedition='2021'\n[lib]\nname='api'\n",
                    ),
                    ("src/lib.rs", "pub mod a;\npub mod c;\n"),
                    ("src/a.rs", "pub mod b;\n"),
                    ("src/a/b.rs", "pub fn value() -> u8 { 7 }\n"),
                    ("src/c.rs", ""),
                    ("runner/Cargo.toml", &manifest),
                    ("runner/src/main.rs", &bin),
                ]);
                let before_root = cargo(&ws.root());
                let before_runner = cargo(&ws.path("runner"));
                let line = if edition == "2015" { 4 } else { 3 };
                let column = bin
                    .lines()
                    .nth(line - 1)
                    .unwrap()
                    .find(if whole { "b::value" } else { "value()" })
                    .unwrap() as u32
                    + 1;
                let target = if whole { "src/c/b.rs" } else { "src/c.rs" };
                let mut record = attempt(
                    &ws,
                    whole,
                    "src/a/b.rs",
                    target,
                    "value",
                    vec![("runner/src/main.rs", line as u32, column)],
                    true,
                )
                .await;
                record["before_compiler"] = json!([before_root, before_runner]);
                record["after_compiler"] = json!([cargo(&ws.root()), cargo(&ws.path("runner"))]);
                let suffix = if whole { "c::b" } else { "c" };
                let expected = format!("::{binding}::{suffix}::value()");
                let passed = record["returned"]["applied"] == true
                    && ws.read("runner/src/main.rs").contains(&expected)
                    && ["before_compiler", "after_compiler"].iter().all(|key| {
                        record[*key]
                            .as_array()
                            .unwrap()
                            .iter()
                            .all(|run| run["passed"] == true)
                    });
                record["expected_binding"] = json!(expected);
                record["passed"] = json!(passed);
                save_named(
                    &format!("ownership-custom-binding-{whole}-{edition}-{renamed}"),
                    record.clone(),
                );
                if !passed {
                    failures.push(record);
                }
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
