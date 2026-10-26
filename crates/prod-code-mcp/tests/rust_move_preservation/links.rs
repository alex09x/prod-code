/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#[cfg(unix)]
use super::{
    cases::{item, module_refs},
    support::*,
};
#[cfg(unix)]
use prod_code_testkit::Workspace;

#[cfg(unix)]
#[tokio::test]
async fn linked_module_alias_cannot_lose_its_shared_physical_source() {
    use std::os::unix::fs::{MetadataExt, symlink};
    let mut failures = Vec::new();
    for directory in [false, true] {
        for whole_module in [false, true] {
            let alias = if directory { "linked/a.rs" } else { "alias.rs" };
            let main = format!(
                "#[path=\"{alias}\"] mod a;\nfn main() {{ assert_eq!(a::f(), 7); }}\n#[test] fn consumer() {{ main(); }}\n"
            );
            let ws = Workspace::new(&[
                (
                    "Cargo.toml",
                    "[package]\nname='move_audit'\nversion='0.1.0'\nedition='2021'\n",
                ),
                ("src/lib.rs", "pub mod a;\npub mod c;\n"),
                ("src/a.rs", "pub fn f() -> u8 { 7 }\n"),
                ("src/c.rs", ""),
                ("src/main.rs", &main),
            ]);
            let link = ws.path(if directory {
                "src/linked"
            } else {
                "src/alias.rs"
            });
            symlink(if directory { "." } else { "a.rs" }, &link).unwrap();
            let source = ws.path("src/a.rs");
            let metadata = std::fs::metadata(&source).unwrap();
            let identity = (metadata.dev(), metadata.ino());
            let before = cargo(&ws.root());
            let mut record = if whole_module {
                module_refs(&ws, "src/a.rs", "src/c/a.rs", Vec::new()).await
            } else {
                item(&ws, "src/a.rs", "src/a/child.rs", "f", 1, 1).await
            };
            record["before_compiler"] = before;
            record["after_compiler"] = cargo(&ws.root());
            record["source_identity"] = serde_json::json!(identity);
            let alias_metadata = std::fs::metadata(ws.path(&format!("src/{alias}"))).ok();
            let after_identity = alias_metadata.map(|m| (m.dev(), m.ino()));
            record["after_alias_identity"] = serde_json::json!(after_identity);
            let id = format!("linked-source-{directory}-{whole_module}");
            save_named(&id, record.clone());
            let safe = record["before_compiler"]["passed"] == true
                && record["returned"].get("error").is_some()
                && record["before"] == record["after"]
                && record["after_compiler"]["passed"] == true
                && after_identity == Some(identity)
                && std::fs::read_link(&link).unwrap()
                    == std::path::Path::new(if directory { "." } else { "a.rs" });
            if !safe {
                failures.push(record);
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[cfg(unix)]
#[tokio::test]
async fn unrelated_linked_module_does_not_refuse_a_regular_move() {
    let ws = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname='move_audit'\nversion='0.1.0'\nedition='2021'\n",
        ),
        (
            "src/lib.rs",
            "pub mod a;\npub mod c;\n#[path=\"alias.rs\"] mod other;\n",
        ),
        ("src/a.rs", "pub const VALUE: u8 = 7;\n"),
        ("src/c.rs", ""),
        ("src/other.rs", "pub const OTHER: u8 = 9;\n"),
    ]);
    std::os::unix::fs::symlink("other.rs", ws.path("src/alias.rs")).unwrap();
    let before = cargo(&ws.root());
    let mut record = module_refs(&ws, "src/a.rs", "src/c/a.rs", Vec::new()).await;
    record["before_compiler"] = before;
    record["after_compiler"] = cargo(&ws.root());
    save_named("unrelated-linked-module", record.clone());
    assert_eq!(record["before_compiler"]["passed"], true, "{record:#}");
    assert_eq!(record["returned"]["applied"], true, "{record:#}");
    assert_eq!(record["after_compiler"]["passed"], true, "{record:#}");
    assert_eq!(ws.read("src/alias.rs"), "pub const OTHER: u8 = 9;\n");
    assert_eq!(
        std::fs::read_link(ws.path("src/alias.rs")).unwrap(),
        std::path::Path::new("other.rs")
    );
}
