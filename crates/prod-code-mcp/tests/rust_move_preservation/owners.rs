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

#[tokio::test]
async fn shared_module_updates_and_executes_every_cargo_consumer() {
    let manifest = "[package]\nname='move_audit'\nversion='0.1.0'\nedition='2024'\n\n[lib]\nname='library_name'\n\n[[bin]]\nname='binary_name'\npath='src/main.rs'\n";
    let lib = "pub mod a;\npub mod c;\npub fn value() -> u8 { a::VALUE }\n#[test] fn library_consumer() { assert_eq!(value(), 7); }\n";
    let bin = "mod a;\nmod c;\nfn main() { assert_eq!(a::VALUE, 7); }\n#[test] fn binary_consumer() { main(); }\n";
    let ws = Workspace::new(&[
        ("Cargo.toml", manifest),
        ("src/lib.rs", lib),
        ("src/main.rs", bin),
        ("src/a.rs", "pub const VALUE: u8 = 7;\n"),
        ("src/c.rs", ""),
    ]);
    let before_compiler = cargo(&ws.root());
    assert_eq!(before_compiler["passed"], true);
    let lib_col = lib.lines().nth(2).unwrap().find("a::VALUE").unwrap() as u32 + 1;
    let bin_col = bin.lines().nth(2).unwrap().find("a::VALUE").unwrap() as u32 + 1;
    let mut record = module_refs(
        &ws,
        "src/a.rs",
        "src/c/a.rs",
        vec![("src/lib.rs", 3, lib_col), ("src/main.rs", 3, bin_col)],
    )
    .await;
    record["before_compiler"] = before_compiler;
    record["after_compiler"] = cargo(&ws.root());
    record["all_roots"] = json!(["src/lib.rs", "src/main.rs"]);
    save_named("all-cargo-consumers", record.clone());
    assert_eq!(record["returned"]["applied"], true, "{record:#}");
    assert_eq!(record["after_compiler"]["passed"], true, "{record:#}");
    assert!(!ws.path("src/a.rs").exists());
    assert!(ws.path("src/c/a.rs").exists());
    assert!(!ws.read("src/lib.rs").contains("mod a;"));
    assert!(!ws.read("src/main.rs").contains("mod a;"));
    assert_eq!(ws.read("src/c.rs").matches("mod a;").count(), 1);
    let diagnostic_uris: Vec<_> = record["analyzer_transcript"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["method"] == "textDocument/diagnostic")
        .filter_map(|r| r["params"]["textDocument"]["uri"].as_str())
        .collect();
    assert!(
        diagnostic_uris
            .iter()
            .any(|uri| uri.ends_with("/src/lib.rs")),
        "{record:#}"
    );
    assert!(
        diagnostic_uris
            .iter()
            .any(|uri| uri.ends_with("/src/main.rs")),
        "{record:#}"
    );
}

#[tokio::test]
async fn shared_item_has_one_child_declaration_and_all_consumers_compile() {
    let manifest = "[package]\nname='move_audit'\nversion='0.1.0'\nedition='2015'\n";
    let ws = Workspace::new(&[
        ("Cargo.toml", manifest),
        ("src/lib.rs", "pub mod shared;\n"),
        ("src/main.rs", "mod shared;\nfn main() {}\n"),
        ("src/shared.rs", "pub fn async() -> u8 { 7 }\n"),
    ]);
    let mut record = item(&ws, "src/shared.rs", "src/shared/util.rs", "async", 1, 1).await;
    std::fs::create_dir_all(ws.path("tests")).unwrap();
    std::fs::write(ws.path("tests/consumer.rs"), "extern crate move_audit;\n#[test] fn consumer() { assert_eq!(move_audit::shared::util::async(), 7); }\n").unwrap();
    record["compiler"] = cargo(&ws.root());
    save_named("shared-item-edition-2015", record.clone());
    assert_eq!(record["returned"]["applied"], true, "{record:#}");
    assert_eq!(record["compiler"]["passed"], true, "{record:#}");
    assert_eq!(ws.read("src/shared.rs").matches("mod util;").count(), 1);
}

#[tokio::test]
async fn force_cannot_accept_two_unknown_owners() {
    let manifest = "[package]\nname='move_audit'\nversion='0.1.0'\nedition='2021'\n";
    let ws = Workspace::new(&[
        ("Cargo.toml", manifest),
        ("src/lib.rs", ""),
        ("src/source.rs", "pub fn api() {}\n"),
        ("src/target.rs", ""),
    ]);
    let record = item(&ws, "src/source.rs", "src/target.rs", "api", 1, 1).await;
    save_named("two-unknown-owners", record.clone());
    assert!(record["returned"].get("error").is_some(), "{record:#}");
    assert_eq!(record["before"], record["after"]);
}

#[tokio::test]
async fn module_paths_mod_rs_and_exact_missing_declaration() {
    let manifest = "[package]\nname='move_audit'\nversion='0.1.0'\nedition='2021'\n";
    for (id, source, lib, c) in [
        (
            "module-path-owner",
            "src/unusual.rs",
            "#[path = \"unusual.rs\"] pub mod a;\npub mod c;\n",
            "",
        ),
        (
            "module-mod-rs",
            "src/a/mod.rs",
            "pub mod a;\npub mod c;\n",
            "",
        ),
        (
            "module-missing-declaration",
            "src/a.rs",
            "/** Original module. */\n#[allow(\n dead_code\n)]\npub mod a;\npub mod c;\n",
            "/** Target declaration. */\n#[allow(dead_code)] pub mod a;\n",
        ),
        (
            "module-nested-decoy",
            "src/a.rs",
            "pub mod a;\npub mod c;\n",
            "mod nested { mod a {} }\n",
        ),
    ] {
        let ws = Workspace::new(&[
            ("Cargo.toml", manifest),
            ("src/lib.rs", lib),
            (source, "pub const VALUE: u8 = 7;\n"),
            ("src/c.rs", c),
        ]);
        let mut record = module_refs(&ws, source, "src/c/a.rs", Vec::new()).await;
        std::fs::create_dir_all(ws.path("tests")).unwrap();
        std::fs::write(
            ws.path("tests/consumer.rs"),
            "#[test] fn consumer() { assert_eq!(move_audit::c::a::VALUE, 7); }\n",
        )
        .unwrap();
        record["compiler"] = cargo(&ws.root());
        save_named(id, record.clone());
        assert_eq!(record["returned"]["applied"], true, "{record:#}");
        assert_eq!(record["compiler"]["passed"], true, "{record:#}");
        assert!(!ws.path(source).exists());
        if id == "module-missing-declaration" {
            assert_eq!(ws.read("src/c.rs").matches("mod a;").count(), 1);
            assert!(ws.read("src/c.rs").contains("/** Original module. */"));
        }
    }
}

#[tokio::test]
async fn one_root_file_can_own_distinct_library_and_binary_targets() {
    let manifest = "[package]\nname='move_audit'\nversion='0.1.0'\nedition='2021'\n[lib]\nname='library_name'\npath='src/root.rs'\n[[bin]]\nname='binary_name'\npath='src/root.rs'\n";
    let ws = Workspace::new(&[
        ("Cargo.toml", manifest),
        ("src/root.rs", "fn main() {}\npub fn api() -> u8 { 7 }\n"),
    ]);
    let mut record = item(&ws, "src/root.rs", "src/util.rs", "api", 2, 2).await;
    std::fs::create_dir_all(ws.path("tests")).unwrap();
    std::fs::write(
        ws.path("tests/consumer.rs"),
        "#[test] fn consumer() { assert_eq!(library_name::util::api(), 7); }\n",
    )
    .unwrap();
    record["compiler"] = cargo(&ws.root());
    save_named("distinct-targets-one-root-file", record.clone());
    assert_eq!(record["returned"]["applied"], true, "{record:#}");
    assert_eq!(record["compiler"]["passed"], true, "{record:#}");
    assert_eq!(ws.read("src/root.rs").matches("mod util;").count(), 1);
}
