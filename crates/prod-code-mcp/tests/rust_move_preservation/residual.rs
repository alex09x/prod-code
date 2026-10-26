/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{
    cases::{item, module_refs},
    support::*,
};
use prod_code_testkit::Workspace;
use serde_json::Value;

const MANIFEST: &str = "[package]\nname='move_audit'\nversion='0.1.0'\nedition='2021'\n";

fn finish(id: &str, ws: &Workspace, before: Value, mut record: Value) -> Value {
    record["before_compiler"] = before;
    record["after_compiler"] = cargo(&ws.root());
    save_named(id, record.clone());
    record
}

#[tokio::test]
async fn residual_attributed_mod_rs_destination_aligns_with_relocated_file() {
    let ws = Workspace::new(&[
        ("Cargo.toml", MANIFEST),
        (
            "src/lib.rs",
            "/** Moved module docs. */\npub mod a;\npub mod c;\n",
        ),
        (
            "src/a/mod.rs",
            "pub mod child;\npub fn f() -> u8 { child::VALUE }\n",
        ),
        ("src/a/child.rs", "pub const VALUE: u8 = 7;\n"),
        ("src/c.rs", ""),
        (
            "tests/consumer.rs",
            "#[test] fn api() { assert_eq!(move_audit::a::f(), 7); assert_eq!(move_audit::a::child::VALUE, 7); }\n",
        ),
    ]);
    let before = cargo(&ws.root());
    ws.write(
        "src/c.rs",
        "/** Destination docs. */\n#[path=\"c/a.rs\"] pub mod a;\n",
    );
    let result = module_refs(&ws, "src/a/mod.rs", "src/c/a.rs", Vec::new()).await;
    if result["returned"]["applied"] == true {
        ws.write(
            "tests/consumer.rs",
            "#[test] fn api() { assert_eq!(move_audit::c::a::f(), 7); assert_eq!(move_audit::c::a::child::VALUE, 7); }\n",
        );
    }
    let record = finish("residual-attributed-mod-rs", &ws, before, result);
    assert_eq!(record["before_compiler"]["passed"], true, "{record:#}");
    assert_eq!(record["returned"]["applied"], true, "{record:#}");
    assert_eq!(record["after_compiler"]["passed"], true, "{record:#}");
    assert!(ws.path("src/c/a/mod.rs").is_file(), "{record:#}");
    assert!(ws.path("src/c/a/child.rs").is_file(), "{record:#}");
    let c_text = ws.read("src/c.rs");
    assert!(c_text.contains("/** Moved module docs. */"), "{c_text}");
    assert!(c_text.contains("/** Destination docs. */"), "{c_text}");
    assert!(c_text.contains("#[path=\"c/a/mod.rs\"]"), "{c_text}");
    assert_eq!(c_text.matches("mod a;").count(), 1, "{c_text}");
}

#[tokio::test]
async fn residual_implicit_workspace_members_participate_in_ownership() {
    let root_manifest = "[workspace]\nmembers = []\n\n[package]\nname='move_audit'\nversion='0.1.0'\nedition='2021'\n\n[dependencies]\nhelper = { path = 'helper' }\n";
    let helper_manifest = "[package]\nname='helper'\nversion='0.1.0'\nedition='2021'\n";
    let mut failures = Vec::new();
    for shared_dest in [true, false] {
        for whole_module in [false, true] {
            let helper_lib = if shared_dest {
                "#[path=\"../../src/a.rs\"] pub mod a;\n#[path=\"../../src/c.rs\"] pub mod c;\n"
            } else {
                "#[path=\"../../src/a.rs\"] pub mod a;\n"
            };
            let ws = Workspace::new(&[
                ("Cargo.toml", root_manifest),
                ("src/lib.rs", "pub mod a;\npub mod c;\n"),
                ("src/a.rs", "pub fn f() -> u8 { 7 }\n"),
                ("src/c.rs", ""),
                ("helper/Cargo.toml", helper_manifest),
                ("helper/src/lib.rs", helper_lib),
                (
                    "helper/tests/consumer.rs",
                    "#[test] fn helper_api() { assert_eq!(helper::a::f(), 7); }\n",
                ),
                (
                    "tests/consumer.rs",
                    "#[test] fn apis() { assert_eq!(move_audit::a::f(), 7); assert_eq!(helper::a::f(), 7); }\n",
                ),
            ]);
            let before = cargo(&ws.root());
            let before_helper = cargo(&ws.path("helper"));
            let target = if whole_module {
                "src/c/a.rs"
            } else if shared_dest {
                "src/c/util.rs"
            } else {
                "src/c.rs"
            };
            let mut result = if whole_module {
                module_refs(&ws, "src/a.rs", target, Vec::new()).await
            } else {
                item(&ws, "src/a.rs", target, "f", 1, 1).await
            };
            if shared_dest && result["returned"]["applied"] == true {
                let path = if whole_module {
                    "c::a::f"
                } else {
                    "c::util::f"
                };
                ws.write(
                    "helper/tests/consumer.rs",
                    &format!("#[test] fn helper_api() {{ assert_eq!(helper::{path}(), 7); }}\n"),
                );
                ws.write(
                    "tests/consumer.rs",
                    &format!(
                        "#[test] fn apis() {{ assert_eq!(move_audit::{path}(), 7); assert_eq!(helper::{path}(), 7); }}\n"
                    ),
                );
            }
            result["before_helper_compiler"] = before_helper;
            result["after_helper_compiler"] = cargo(&ws.path("helper"));
            let id = format!("residual-implicit-workspace-{shared_dest}-{whole_module}");
            let record = finish(&id, &ws, before, result);
            let ok = if shared_dest {
                record["before_compiler"]["passed"] == true
                    && record["before_helper_compiler"]["passed"] == true
                    && record["returned"]["applied"] == true
                    && record["after_compiler"]["passed"] == true
                    && record["after_helper_compiler"]["passed"] == true
            } else {
                record["before_compiler"]["passed"] == true
                    && record["before_helper_compiler"]["passed"] == true
                    && record["returned"].get("error").is_some()
                    && record["before"] == record["after"]
                    && record["after_compiler"]["passed"] == true
                    && record["after_helper_compiler"]["passed"] == true
            };
            if !ok {
                failures.push(record);
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[tokio::test]
async fn residual_cfg_attr_path_aliases_and_controls() {
    let mut failures = Vec::new();
    for whole_module in [false, true] {
        let ws = Workspace::new(&[
            ("Cargo.toml", MANIFEST),
            (
                "src/lib.rs",
                "pub mod a;\npub mod c;\n#[cfg_attr(all(), path=\"a.rs\")] pub mod alias;\n",
            ),
            ("src/a.rs", "pub fn f() -> u8 { 7 }\n"),
            ("src/c.rs", ""),
            (
                "tests/consumer.rs",
                "#[test] fn apis() { assert_eq!(move_audit::a::f(), 7); assert_eq!(move_audit::alias::f(), 7); }\n",
            ),
        ]);
        let before = cargo(&ws.root());
        let result = if whole_module {
            module_refs(&ws, "src/a.rs", "src/c/a.rs", Vec::new()).await
        } else {
            item(&ws, "src/a.rs", "src/c.rs", "f", 1, 1).await
        };
        let record = finish(
            &format!("residual-cfg-attr-alias-{whole_module}"),
            &ws,
            before,
            result,
        );
        if !(record["before_compiler"]["passed"] == true
            && record["returned"].get("error").is_some()
            && record["before"] == record["after"]
            && record["after_compiler"]["passed"] == true)
        {
            failures.push(record);
        }
    }

    let ws = Workspace::new(&[
        ("Cargo.toml", MANIFEST),
        (
            "src/lib.rs",
            "#[cfg_attr(all(), allow(dead_code))]\npub mod a;\npub mod c;\n",
        ),
        ("src/a.rs", "pub fn f() -> u8 { 7 }\n"),
        ("src/c.rs", ""),
        (
            "tests/consumer.rs",
            "#[test] fn api() { assert_eq!(move_audit::a::f(), 7); }\n",
        ),
    ]);
    let before = cargo(&ws.root());
    let result = module_refs(&ws, "src/a.rs", "src/c/a.rs", Vec::new()).await;
    if result["returned"]["applied"] == true {
        ws.write(
            "tests/consumer.rs",
            "#[test] fn api() { assert_eq!(move_audit::c::a::f(), 7); }\n",
        );
    }
    let record = finish("residual-cfg-attr-non-path-control", &ws, before, result);
    if !(record["before_compiler"]["passed"] == true
        && record["returned"]["applied"] == true
        && record["after_compiler"]["passed"] == true
        && ws
            .read("src/c.rs")
            .contains("#[cfg_attr(all(), allow(dead_code))]"))
    {
        failures.push(record);
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
