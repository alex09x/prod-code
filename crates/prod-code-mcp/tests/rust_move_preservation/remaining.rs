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
use serde_json::json;

const MANIFEST: &str = "[package]\nname='move_audit'\nversion='0.1.0'\nedition='2021'\n";

#[tokio::test]
async fn remaining_descendant_path_bindings_cannot_silently_change() {
    let mut failures = Vec::new();
    for (id, entry, body, child, decoy, setup) in [
        (
            "attributed-entry",
            "a_impl.rs",
            "#[path=\"shared.rs\"] pub mod child;\n",
            "src/shared.rs",
            "src/c/shared.rs",
            None,
        ),
        (
            "moved-descendant",
            "a.rs",
            "pub mod nested;\n",
            "src/shared.rs",
            "src/c/shared.rs",
            Some((
                "src/a/nested.rs",
                "#[path=\"../shared.rs\"] pub mod child;\n",
            )),
        ),
        (
            "inline-descendant",
            "a_impl.rs",
            "pub mod inner { #[path=\"../shared.rs\"] pub mod child; }\n",
            "src/shared.rs",
            "src/c/shared.rs",
            None,
        ),
    ] {
        let lib = if id == "moved-descendant" {
            "pub mod a;\npub mod c;\n".to_string()
        } else {
            format!("#[path=\"{entry}\"] pub mod a;\npub mod c;\n")
        };
        let path = format!("src/{entry}");
        let ws = Workspace::new(&[
            ("Cargo.toml", MANIFEST),
            ("src/lib.rs", &lib),
            (&path, body),
            ("src/c.rs", ""),
            (child, "pub const VALUE: u8 = 7;\n"),
            (decoy, "pub const VALUE: u8 = 99;\n"),
        ]);
        if let Some((path, text)) = setup {
            ws.write(path, text);
        }
        // Rust's lexical inline-module path lookup requires these intermediate directories.
        if id == "inline-descendant" {
            std::fs::create_dir_all(ws.path("src/inner")).unwrap();
            std::fs::create_dir_all(ws.path("src/c/inner")).unwrap();
        }
        let suffix = match id {
            "moved-descendant" => "nested::child",
            "inline-descendant" => "inner::child",
            _ => "child",
        };
        let consumer = |module: &str| {
            format!(
                "#[test] fn value() {{ assert_eq!(move_audit::{module}::{suffix}::VALUE, 7); }}\n"
            )
        };
        ws.write("tests/consumer.rs", &consumer("a"));
        let before = cargo(&ws.root());
        let mut record = module_refs(&ws, &path, "src/c/a.rs", Vec::new()).await;
        record["before_compiler"] = before;
        // A rebound decoy compiles, so execute a downstream value assertion too.
        if record["returned"]["applied"] == true {
            ws.write("tests/consumer.rs", &consumer("c::a"));
        }
        record["after_compiler"] = cargo(&ws.root());
        save_named(&format!("remaining-path-{id}"), record.clone());
        if !(record["before_compiler"]["passed"] == true
            && record["returned"].get("error").is_some()
            && record["before"] == record["after"]
            && record["after_compiler"]["passed"] == true)
        {
            failures.push(record);
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[tokio::test]
async fn remaining_relocated_path_target_keeps_supported_moves() {
    let ws = Workspace::new(&[
        ("Cargo.toml", MANIFEST),
        ("src/lib.rs", "pub mod a;\npub mod c;\n"),
        ("src/a.rs", "#[path=\"a/shared.rs\"] pub mod child;\n"),
        ("src/a/shared.rs", "pub const VALUE: u8 = 7;\n"),
        ("src/c.rs", ""),
        (
            "tests/consumer.rs",
            "#[test] fn value() { assert_eq!(move_audit::a::child::VALUE, 7); }\n",
        ),
    ]);
    let before = cargo(&ws.root());
    let mut record = module_refs(&ws, "src/a.rs", "src/c/a.rs", Vec::new()).await;
    ws.write(
        "tests/consumer.rs",
        "#[test] fn value() { assert_eq!(move_audit::c::a::child::VALUE, 7); }\n",
    );
    record["before_compiler"] = before;
    record["after_compiler"] = cargo(&ws.root());
    save_named("remaining-supported-path", record.clone());
    assert_eq!(record["before_compiler"]["passed"], true, "{record:#}");
    assert_eq!(record["returned"]["applied"], true, "{record:#}");
    assert_eq!(record["after_compiler"]["passed"], true, "{record:#}");
}

#[tokio::test]
async fn remaining_attributed_module_children_have_their_actual_owner() {
    let ws = Workspace::new(&[
        ("Cargo.toml", MANIFEST),
        (
            "src/lib.rs",
            "#[path=\"container.rs\"] pub mod a;\npub mod c;\n",
        ),
        ("src/container.rs", "pub mod child;\n"),
        ("src/child.rs", "pub fn f() -> u8 { 7 }\n"),
        ("src/c.rs", ""),
        (
            "tests/consumer.rs",
            "#[test] fn value() { assert_eq!(move_audit::a::child::f(), 7); }\n",
        ),
    ]);
    let before = cargo(&ws.root());
    let mut record = item(&ws, "src/child.rs", "src/c.rs", "f", 1, 1).await;
    ws.write(
        "tests/consumer.rs",
        "#[test] fn value() { assert_eq!(move_audit::c::f(), 7); }\n",
    );
    record["before_compiler"] = before;
    record["after_compiler"] = cargo(&ws.root());
    save_named("remaining-attributed-child-owner", record.clone());
    assert_eq!(record["before_compiler"]["passed"], true, "{record:#}");
    assert_eq!(record["returned"]["applied"], true, "{record:#}");
    assert_eq!(record["after_compiler"]["passed"], true, "{record:#}");
}

#[cfg(unix)]
#[tokio::test]
async fn remaining_linked_directory_descendants_are_owned_even_when_forced() {
    use std::os::unix::fs::{MetadataExt, symlink};
    let mut failures = Vec::new();
    for whole_module in [false, true] {
        let ws = Workspace::new(&[
            ("Cargo.toml", MANIFEST),
            ("src/lib.rs", "pub mod a;\npub mod c;\n"),
            ("src/a/mod.rs", "pub mod child;\n"),
            ("src/a/child.rs", "pub mod deep;\n"),
            ("src/a/child/deep.rs", "pub fn f() -> u8 { 7 }\n"),
            ("src/c.rs", ""),
            (
                "src/main.rs",
                "#[path=\"linked/mod.rs\"] mod alias;\nfn main() { assert_eq!(alias::child::deep::f(), 7); }\n#[test] fn consumer() { main(); }\n",
            ),
        ]);
        symlink("a", ws.path("src/linked")).unwrap();
        let source = ws.path("src/a/child/deep.rs");
        let meta = std::fs::metadata(&source).unwrap();
        let identity = (meta.dev(), meta.ino());
        let before = cargo(&ws.root());
        let mut record = if whole_module {
            module_refs(&ws, "src/a/child/deep.rs", "src/c/deep.rs", Vec::new()).await
        } else {
            item(&ws, "src/a/child/deep.rs", "src/c/moved.rs", "f", 1, 1).await
        };
        record["before_compiler"] = before;
        record["after_compiler"] = cargo(&ws.root());
        let after_identity = std::fs::metadata(ws.path("src/linked/child/deep.rs"))
            .ok()
            .map(|m| (m.dev(), m.ino()));
        record["before_identity"] = json!(identity);
        record["after_identity"] = json!(after_identity);
        save_named(
            &format!("remaining-linked-descendant-{whole_module}"),
            record.clone(),
        );
        if !(record["before_compiler"]["passed"] == true
            && record["returned"].get("error").is_some()
            && record["before"] == record["after"]
            && record["after_compiler"]["passed"] == true
            && after_identity == Some(identity)
            && std::fs::read_link(ws.path("src/linked")).unwrap() == std::path::Path::new("a"))
        {
            failures.push(record);
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
