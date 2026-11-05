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
use serde_json::{Value, json};
use std::process::Command;

fn configurations(ws: &Workspace) -> Value {
    let mut runs = vec![cargo(&ws.root())];
    let output = Command::new("cargo")
        .args([
            "test",
            "--offline",
            "--all-targets",
            "--quiet",
            "--features",
            "enabled",
            "--manifest-path",
        ])
        .arg(ws.path("Cargo.toml"))
        .output()
        .unwrap();
    runs.push(json!({"passed":output.status.success(), "commands":[{
        "argv":["cargo", "test", "--offline", "--all-targets", "--quiet", "--features", "enabled", "--manifest-path", "<fixture>/Cargo.toml"],
        "exit": output.status.code(), "stdout":String::from_utf8_lossy(&output.stdout), "stderr":String::from_utf8_lossy(&output.stderr)
    }], "sources":snapshot(&ws.root())}));
    json!({"passed":runs.iter().all(|r| r["passed"] == true), "runs":runs})
}

#[tokio::test]
async fn review_conditional_membership_checks_feature_on_and_off() {
    let mut failures = Vec::new();
    for whole in [false, true] {
        for matched in [false, true] {
            let source_cfg = if matched {
                "#[cfg(feature=\"enabled\")] "
            } else {
                ""
            };
            let lib =
                format!("{source_cfg}pub mod source;\n#[cfg(feature=\"enabled\")] pub mod dest;\n");
            let probe_cfg = if matched {
                "#[cfg(feature=\"enabled\")] "
            } else {
                ""
            };
            let probe = format!("{probe_cfg}#[test] fn api() {{ let _ = p::source::Api; }}\n");
            let bin = if matched {
                format!(
                    "{source_cfg}mod source;\n#[cfg(feature=\"enabled\")] mod dest;\nfn main() {{}}\n"
                )
            } else {
                "mod source;\nmod dest;\nfn main() {}\n".to_owned()
            };
            let ws = Workspace::new(&[
                (
                    "Cargo.toml",
                    "[package]\nname='p'\nversion='0.1.0'\nedition='2021'\n[features]\nenabled=[]\n",
                ),
                ("src/lib.rs", &lib),
                ("src/main.rs", &bin),
                ("src/source.rs", "pub struct Api;\n"),
                ("src/dest.rs", ""),
                ("tests/api.rs", &probe),
            ]);
            let before = configurations(&ws);
            let target = if whole {
                "src/dest/source.rs"
            } else {
                "src/dest.rs"
            };
            let mut record =
                attempt(&ws, whole, "src/source.rs", target, "Api", vec![], true).await;
            if record["returned"]["applied"] == true {
                let api = if whole { "dest::source" } else { "dest" };
                ws.write(
                    "tests/api.rs",
                    &format!("{probe_cfg}#[test] fn api() {{ let _ = p::{api}::Api; }}\n"),
                );
            }
            record["before_configurations"] = before;
            record["after_configurations"] = configurations(&ws);
            save_named(
                &format!("ownership-features-{whole}-{matched}"),
                record.clone(),
            );
            let behavior = if matched {
                record["returned"]["applied"] == true
            } else {
                record["returned"].get("error").is_some() && record["before"] == record["after"]
            };
            if !(behavior
                && record["before_configurations"]["passed"] == true
                && record["after_configurations"]["passed"] == true)
            {
                failures.push(record);
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[tokio::test]
async fn review_renamed_extern_binding_retains_library_identity() {
    let mut failures = Vec::new();
    for whole in [false, true] {
        for edition in ["2015", "2021"] {
            let dependency = if edition == "2015" {
                "extern crate renamed as binding;\n"
            } else {
                ""
            };
            let binding = if edition == "2015" {
                "binding"
            } else {
                "renamed"
            };
            let bin = format!(
                "{dependency}mod c {{ pub fn value() -> u8 {{ 9 }} pub mod b {{ pub fn value() -> u8 {{ 9 }} }} }}\nfn main() {{ assert_eq!({binding}::a::b::value(), 7); }}\n#[test] fn runtime() {{ main(); }}\n"
            );
            let manifest = format!(
                "[package]\nname='runner'\nversion='0.1.0'\nedition='{edition}'\n[dependencies]\nrenamed={{path='..',package='p'}}\n[[bin]]\nname='p'\npath='src/main.rs'\n"
            );
            let ws = Workspace::new(&[
                (
                    "Cargo.toml",
                    "[workspace]\nmembers=['runner']\n[package]\nname='p'\nversion='0.1.0'\nedition='2021'\n",
                ),
                ("src/lib.rs", "pub mod a;\npub mod c;\n"),
                ("src/a.rs", "pub mod b;\n"),
                ("src/a/b.rs", "pub fn value() -> u8 { 7 }\n"),
                ("src/c.rs", ""),
                ("runner/Cargo.toml", &manifest),
                ("runner/src/main.rs", &bin),
            ]);
            let before = cargo(&ws.path("runner"));
            let line = if edition == "2015" { 3 } else { 2 };
            let col = bin
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
                vec![("runner/src/main.rs", line as u32, col)],
                true,
            )
            .await;
            record["before_compiler"] = before;
            record["after_compiler"] = cargo(&ws.path("runner"));
            save_named(
                &format!("ownership-renamed-{whole}-{edition}"),
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

#[tokio::test]
async fn review_conditional_alias_refuses_only_affected_files_and_descendants() {
    let mut failures = Vec::new();
    for whole in [false, true] {
        let ws = Workspace::new(&[
            (
                "Cargo.toml",
                "[package]\nname='p'\nversion='0.1.0'\nedition='2021'\n[features]\nenabled=[]\n",
            ),
            (
                "src/lib.rs",
                "pub mod a;\npub mod c;\n#[cfg_attr(feature=\"enabled\", path=\"a/child.rs\")] pub mod alias;\n",
            ),
            (
                "src/a.rs",
                "pub mod child;\npub fn f() -> u8 { child::VALUE }\n",
            ),
            ("src/a/child.rs", "pub const VALUE: u8 = 7;\n"),
            ("src/alias.rs", "pub const VALUE: u8 = 9;\n"),
            ("src/c.rs", ""),
            (
                "tests/api.rs",
                "#[test] fn api() { assert_eq!(p::a::f(), 7); assert_eq!(p::a::child::VALUE, 7); }\n",
            ),
        ]);
        let before = configurations(&ws);
        let source = if whole { "src/a.rs" } else { "src/a/child.rs" };
        let target = if whole { "src/c/a.rs" } else { "src/c.rs" };
        let mut record = attempt(&ws, whole, source, target, "VALUE", vec![], true).await;
        record["before_configurations"] = before;
        record["after_configurations"] = configurations(&ws);
        save_named(
            &format!("ownership-conditional-alias-{whole}"),
            record.clone(),
        );
        if !(record["before_configurations"]["passed"] == true
            && record["returned"].get("error").is_some()
            && record["before"] == record["after"]
            && record["after_configurations"]["passed"] == true)
        {
            failures.push(record);
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[tokio::test]
async fn review_known_literal_cfg_attr_owners_preserve_metadata_and_api() {
    let mut failures = Vec::new();
    for whole in [false, true] {
        for (index, attribute, source) in [
            (
                0,
                "#[cfg_attr(all(), path=\"odd.rs\", allow(dead_code))]",
                "src/odd.rs",
            ),
            (
                1,
                "#[cfg_attr(not(any()), cfg_attr(all(), path=r#\"odd.rs\"#))]",
                "src/odd.rs",
            ),
            (2, "#[cfg_attr(any(), path=\"inactive.rs\")]", "src/a.rs"),
        ] {
            let lib = format!(
                "/** literal odd.rs remains in docs. */\n{attribute} pub mod a;\npub mod c;\n"
            );
            let ws = Workspace::new(&[
                (
                    "Cargo.toml",
                    "[package]\nname='p'\nversion='0.1.0'\nedition='2021'\n",
                ),
                ("src/lib.rs", &lib),
                (source, "pub fn f() -> &'static str { r#\"odd.rs\"# }\n"),
                ("src/c.rs", ""),
                (
                    "tests/api.rs",
                    "#[test] fn api() { assert_eq!(p::a::f(), \"odd.rs\"); }\n",
                ),
            ]);
            let before = cargo(&ws.root());
            let target = if whole { "src/c/a.rs" } else { "src/c.rs" };
            let mut record = attempt(&ws, whole, source, target, "f", vec![], true).await;
            if record["returned"]["applied"] == true {
                let api = if whole { "c::a" } else { "c" };
                ws.write(
                    "tests/api.rs",
                    &format!("#[test] fn api() {{ assert_eq!(p::{api}::f(), \"odd.rs\"); }}\n"),
                );
            }
            record["before_compiler"] = before;
            record["after_compiler"] = cargo(&ws.root());
            save_named(
                &format!("ownership-known-literal-{whole}-{index}"),
                record.clone(),
            );
            let docs_file = if whole { "src/c.rs" } else { "src/lib.rs" };
            if !(record["before_compiler"]["passed"] == true
                && record["returned"]["applied"] == true
                && record["after_compiler"]["passed"] == true
                && ws
                    .read(docs_file)
                    .contains("literal odd.rs remains in docs."))
            {
                failures.push(record);
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
