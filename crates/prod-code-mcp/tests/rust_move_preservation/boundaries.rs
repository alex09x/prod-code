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
use serde_json::{Value, json};

const MANIFEST: &str = "[package]\nname='move_audit'\nversion='0.1.0'\nedition='2021'\n";

fn finish(id: &str, ws: &Workspace, before: Value, mut record: Value) -> Value {
    record["before_compiler"] = before;
    record["after_compiler"] = cargo(&ws.root());
    save_named(id, record.clone());
    record
}

#[tokio::test]
async fn boundary_inline_path_owners_preserve_every_api() {
    let mut failures = Vec::new();
    for nested in [false, true] {
        for whole_module in [false, true] {
            let owner = if nested { "src/outer.rs" } else { "src/lib.rs" };
            let source = "src/real/shared.rs";
            let real = if nested {
                "src/real/mod.rs"
            } else {
                "src/real.rs"
            };
            let body = if nested {
                "#[path=\"real/mod.rs\"] pub mod real;\n#[path=\"real\"] pub mod alias { pub mod shared; }\n"
            } else {
                "pub mod real;\n#[path=\"real\"] pub mod alias { pub mod shared; }\n"
            };
            let prefix = if nested { "outer::" } else { "" };
            let consumer = format!(
                "#[test] fn apis() {{ assert_eq!(move_audit::{prefix}real::shared::f(), 7); assert_eq!(move_audit::{prefix}alias::shared::f(), 7); }}\n"
            );
            let ws = Workspace::new(&[
                ("Cargo.toml", MANIFEST),
                ("src/lib.rs", "pub mod outer;\npub mod dest;\n"),
                (owner, body),
                (real, "pub mod shared;\n"),
                (source, "pub fn f() -> u8 { 7 }\n"),
                ("src/dest.rs", ""),
                ("tests/consumer.rs", &consumer),
            ]);
            if !nested {
                ws.write("src/lib.rs", &format!("{}pub mod dest;\n", ws.read(owner)));
            }
            let before = cargo(&ws.root());
            let result = if whole_module {
                module_refs(&ws, source, "src/dest/shared.rs", Vec::new()).await
            } else {
                item(&ws, source, "src/dest.rs", "f", 1, 1).await
            };
            let record = finish(
                &format!("boundary-inline-{nested}-{whole_module}"),
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
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[tokio::test]
async fn boundary_single_inline_path_owner_still_supports_moves() {
    let mut failures = Vec::new();
    for nested in [false, true] {
        for whole_module in [false, true] {
            let (body, prefix) = if nested {
                (
                    "pub mod container { #[path=\"../real\"] pub mod alias { pub mod shared; } }\n",
                    "container::alias",
                )
            } else {
                (
                    "#[path=\"real\"] pub mod alias { pub mod shared; }\n",
                    "alias",
                )
            };
            let ws = Workspace::new(&[
                ("Cargo.toml", MANIFEST),
                ("src/lib.rs", &format!("{body}pub mod dest;\n")),
                ("src/real/shared.rs", "pub fn f() -> u8 { 7 }\n"),
                ("src/dest.rs", ""),
                (
                    "tests/consumer.rs",
                    &format!(
                        "#[test] fn api() {{ assert_eq!(move_audit::{prefix}::shared::f(), 7); }}\n"
                    ),
                ),
            ]);
            std::fs::create_dir_all(ws.path("src/container")).unwrap();
            let before = cargo(&ws.root());
            let result = if whole_module {
                module_refs(&ws, "src/real/shared.rs", "src/dest/shared.rs", Vec::new()).await
            } else {
                item(&ws, "src/real/shared.rs", "src/dest.rs", "f", 1, 1).await
            };
            let suffix = if whole_module { "shared::f" } else { "f" };
            if result["returned"]["applied"] == true {
                ws.write(
                    "tests/consumer.rs",
                    &format!(
                        "#[test] fn api() {{ assert_eq!(move_audit::dest::{suffix}(), 7); }}\n"
                    ),
                );
            }
            let record = finish(
                &format!("boundary-single-inline-{nested}-{whole_module}"),
                &ws,
                before,
                result,
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

#[cfg(unix)]
#[tokio::test]
async fn boundary_symlinks_are_checked_before_parent_normalization() {
    use std::os::unix::fs::{MetadataExt, symlink};
    let mut failures = Vec::new();
    for whole_module in [false, true] {
        let ws = Workspace::new(&[
            ("Cargo.toml", MANIFEST),
            (
                "src/lib.rs",
                "pub mod real;\npub mod dest;\n#[path=\"linked/../shared.rs\"] pub mod alias;\n",
            ),
            ("src/real.rs", "pub mod shared;\n"),
            ("src/real/shared.rs", "pub fn f() -> u8 { 7 }\n"),
            ("src/shared.rs", "pub fn f() -> u8 { 99 }\n"),
            ("src/dest.rs", ""),
            (
                "src/main.rs",
                "fn main() { assert_eq!(move_audit::alias::f(), 7); }\n#[test] fn consumer() { main(); }\n",
            ),
            (
                "tests/consumer.rs",
                "#[test] fn apis() { assert_eq!(move_audit::real::shared::f(), 7); assert_eq!(move_audit::alias::f(), 7); }\n",
            ),
        ]);
        std::fs::create_dir_all(ws.path("src/real/sub")).unwrap();
        symlink("real/sub", ws.path("src/linked")).unwrap();
        let identity = |path: &str| {
            std::fs::metadata(ws.path(path))
                .ok()
                .map(|m| (m.dev(), m.ino()))
        };
        let before_identity = identity("src/linked/../shared.rs");
        assert_eq!(before_identity, identity("src/real/shared.rs"));
        assert_ne!(before_identity, identity("src/shared.rs"));
        let before = cargo(&ws.root());
        let result = if whole_module {
            module_refs(&ws, "src/real/shared.rs", "src/dest/shared.rs", Vec::new()).await
        } else {
            item(&ws, "src/real/shared.rs", "src/dest.rs", "f", 1, 1).await
        };
        let mut record = finish(
            &format!("boundary-symlink-{whole_module}"),
            &ws,
            before,
            result,
        );
        record["before_identity"] = json!(before_identity);
        record["after_identity"] = json!(identity("src/linked/../shared.rs"));
        record["decoy_identity"] = json!(identity("src/shared.rs"));
        save_named(&format!("boundary-symlink-{whole_module}"), record.clone());
        if !(record["before_compiler"]["passed"] == true
            && record["returned"].get("error").is_some()
            && record["before"] == record["after"]
            && record["after_compiler"]["passed"] == true
            && identity("src/linked/../shared.rs") == before_identity
            && std::fs::read_link(ws.path("src/linked")).unwrap()
                == std::path::Path::new("real/sub"))
        {
            failures.push(record);
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[tokio::test]
async fn boundary_new_modules_choose_conventional_parents() {
    let mut failures = Vec::new();
    for binary in [false, true] {
        let root = if binary { "src/main.rs" } else { "src/lib.rs" };
        let source = "#[path=\"extra.rs\"] mod extra;\npub fn f() -> u8 { 7 }\n";
        let source = if binary {
            format!(
                "{source}fn main() {{ assert_eq!(f(), 7); }}\n#[test] fn consumer() {{ main(); }}\n"
            )
        } else {
            source.to_string()
        };
        let ws = Workspace::new(&[
            ("Cargo.toml", MANIFEST),
            (root, &source),
            ("src/extra.rs", ""),
        ]);
        if !binary {
            ws.write("src/main.rs", "fn main() { assert_eq!(move_audit::f(), 7); }\n#[test] fn consumer() { main(); }\n");
            ws.write(
                "tests/consumer.rs",
                "#[test] fn consumer() { assert_eq!(move_audit::f(), 7); }\n",
            );
        }
        let before = cargo(&ws.root());
        let result = item(&ws, root, "src/util.rs", "f", 2, 2).await;
        // The scripted analyzer deliberately reports no references. Update the
        // executable consumer separately, retaining the exact returned sources.
        if result["returned"]["applied"] == true {
            if binary {
                ws.write(
                    root,
                    &ws.read(root)
                        .replace("assert_eq!(f(),", "assert_eq!(util::f(),"),
                );
            } else {
                ws.write("src/main.rs", "fn main() { assert_eq!(move_audit::util::f(), 7); }\n#[test] fn consumer() { main(); }\n");
                ws.write(
                    "tests/consumer.rs",
                    "#[test] fn consumer() { assert_eq!(move_audit::util::f(), 7); }\n",
                );
            }
        }
        let record = finish(&format!("boundary-parent-{binary}"), &ws, before, result);
        if !(record["before_compiler"]["passed"] == true
            && record["returned"]["applied"] == true
            && record["after_compiler"]["passed"] == true
            && ws.read(root).contains("pub mod util;")
            && ws.read("src/extra.rs").is_empty())
        {
            failures.push(record);
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
