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
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

const MANIFEST: &str = "[package]\nname='move_audit'\nversion='0.1.0'\nedition='2021'\n";

pub(super) async fn item(
    ws: &Workspace,
    source: &str,
    target: &str,
    name: &str,
    line: u32,
    end: u32,
) -> Value {
    let name = name.to_string();
    let trace = Arc::new(Mutex::new(Vec::new()));
    let recorded = trace.clone();
    let gateway = ScriptedGateway::start(move |method, params| {
        let response = match method {
            "textDocument/documentSymbol" => {
                json!([answers::document_symbol(&name, 12, line, end, 1)])
            }
            "textDocument/references" => json!([]),
            "textDocument/diagnostic" => answers::no_diagnostics(),
            _ => Value::Null,
        };
        recorded
            .lock()
            .unwrap()
            .push(json!({"method": method, "params": params, "response": response}));
        response
    })
    .await;
    let before = snapshot(&ws.root());
    let result = move_item(
        gateway.addr(),
        &ws.root(),
        &ws.path(source),
        line,
        1,
        &ws.path(target),
        true,
        true,
    )
    .await;
    let returned = match result {
        Ok(plan) => {
            json!({"applied": plan.applied, "sources": plan.rewritten, "diagnostics": plan.diagnostics})
        }
        Err(e) => json!({"error": format!("{e:#}")}),
    };
    json!({"before": before, "returned": returned, "after": snapshot(&ws.root()),
        "provenance": "scripted exact documentSymbol ranges, empty references and clean diagnostics; independent real cargo test --all-targets",
        "analyzer_transcript": *trace.lock().unwrap()})
}

async fn module(ws: &Workspace, source: &str, target: &str) -> Value {
    module_refs(ws, source, target, Vec::new()).await
}

pub(super) async fn module_refs(
    ws: &Workspace,
    source: &str,
    target: &str,
    refs: Vec<(&str, u32, u32)>,
) -> Value {
    let trace = Arc::new(Mutex::new(Vec::new()));
    let recorded = trace.clone();
    let mut references = json!([]);
    for (file, line, col) in refs {
        references.as_array_mut().unwrap().extend(
            answers::locations(&ws.path(file), &[(line, col)])
                .as_array()
                .unwrap()
                .clone(),
        );
    }
    let gateway = ScriptedGateway::start(move |method, params| {
        let response = match method {
            "textDocument/references" => references.clone(),
            "textDocument/diagnostic" => answers::no_diagnostics(),
            _ => Value::Null,
        };
        recorded
            .lock()
            .unwrap()
            .push(json!({"method": method, "params": params, "response": response}));
        response
    })
    .await;
    let before = snapshot(&ws.root());
    let returned = match move_module(
        gateway.addr(),
        &ws.root(),
        &ws.path(source),
        &ws.path(target),
    )
    .await
    {
        Ok(mut plan) => {
            let result = plan.write(true).map_err(|e| format!("{e:#}"));
            json!({"applied": plan.applied, "sources": plan.rewritten, "removed": plan.removed, "result": result})
        }
        Err(e) => json!({"error": format!("{e:#}")}),
    };
    json!({"before": before, "returned": returned, "after": snapshot(&ws.root()),
        "provenance": "scripted explicitly recorded references and clean diagnostics; independent real cargo test --all-targets",
        "analyzer_transcript": *trace.lock().unwrap()})
}

fn finish(id: &str, ws: &Workspace, mut record: Value) {
    record["compiler"] = cargo(&ws.root());
    save_named(id, record.clone());
    assert_eq!(record["returned"]["applied"], true, "{record:#}");
    assert_eq!(record["compiler"]["passed"], true, "{record:#}");
}

#[tokio::test]
async fn item_docs_split_visibility_and_nested_imports() {
    let source = "/** A block doc.\n * super::text stays.\n */\n#[allow(\n dead_code\n)]\npub\nfn api() -> &'static str {\n let _ = std::path::Path::new(\".\");\n r#\"literal\nsuper::text\"#\n}\n";
    let ws = Workspace::new(&[("Cargo.toml", MANIFEST), ("src/lib.rs", source)]);
    let record = item(&ws, "src/lib.rs", "src/util.rs", "api", 8, 12).await;
    std::fs::create_dir_all(ws.path("tests")).unwrap();
    std::fs::write(
        ws.path("tests/consumer.rs"),
        "#[test] fn api() { assert_eq!(move_audit::util::api(), \"literal\\nsuper::text\"); }\n",
    )
    .unwrap();
    assert!(ws.read("src/util.rs").starts_with("/** A block doc."));
    assert!(ws.read("src/util.rs").contains("pub\nfn api"));
    finish("split-visibility-block-doc", &ws, record);

    let source = "use std::{path::Path, collections::HashMap};\npub fn api() { let _ = Path::new(\".\"); }\n";
    let ws = Workspace::new(&[
        ("Cargo.toml", MANIFEST),
        ("src/lib.rs", "pub mod source;\npub mod dest;\n"),
        ("src/source.rs", source),
        ("src/dest.rs", ""),
    ]);
    let record = item(&ws, "src/source.rs", "src/dest.rs", "api", 2, 2).await;
    assert!(ws.read("src/dest.rs").contains("use std::path::Path;"));
    assert!(!ws.read("src/dest.rs").contains("HashMap"));
    finish("nested-import-positive", &ws, record);
}

#[tokio::test]
async fn exact_path_owner_and_nested_filename_decoys() {
    for (id, declaration, target) in [
        (
            "same-filename-decoy",
            "#[path = \"other/util.rs\"] mod other;\n",
            "src/util.rs",
        ),
        (
            "nested-declaration-decoy",
            "mod nested { mod util {} }\n",
            "src/util.rs",
        ),
        (
            "exact-missing-path",
            "#[allow(dead_code)] #[path = \"deep/util.rs\"] mod actual;\n",
            "src/deep/util.rs",
        ),
        (
            "nested-missing-declaration",
            "mod nested { #[path = \"util.rs\"] mod actual; }\n",
            "src/nested/util.rs",
        ),
    ] {
        let source = format!("{declaration}pub fn api() -> u8 {{ 7 }}\n");
        let ws = Workspace::new(&[
            ("Cargo.toml", MANIFEST),
            ("src/lib.rs", &source),
            ("src/other/util.rs", ""),
        ]);
        let record = item(&ws, "src/lib.rs", target, "api", 2, 2).await;
        if id == "exact-missing-path" || id == "nested-missing-declaration" {
            assert_eq!(ws.read("src/lib.rs").matches("mod actual;").count(), 1);
            assert!(!ws.read("src/lib.rs").contains("mod util;"));
        } else {
            assert!(ws.read("src/lib.rs").contains("pub mod util;"));
        }
        finish(id, &ws, record);
    }
}

#[tokio::test]
async fn actual_custom_cargo_roots_and_unknown_force() {
    let manifest = format!(
        "{MANIFEST}\n[lib]\nname='library_name'\npath='source/library.rs'\n\n[[bin]]\nname='binary_name'\npath='source/program.rs'\n"
    );
    let ws = Workspace::new(&[
        ("Cargo.toml", &manifest),
        ("source/library.rs", "pub fn api() -> u8 { 7 }\n"),
        ("source/program.rs", "fn main() {}\n"),
    ]);
    let mut record = item(&ws, "source/library.rs", "source/util.rs", "api", 1, 1).await;
    record["custom_root_after"] =
        json!({"library": ws.read("source/library.rs"), "binary": ws.read("source/program.rs")});
    assert!(ws.read("source/library.rs").contains("pub mod util;"));
    assert_eq!(ws.read("source/program.rs"), "fn main() {}\n");
    std::fs::create_dir_all(ws.path("tests")).unwrap();
    std::fs::write(
        ws.path("tests/consumer.rs"),
        "#[test] fn api() { assert_eq!(library_name::util::api(), 7); }\n",
    )
    .unwrap();
    finish("custom-cargo-roots", &ws, record);

    for (id, target) in [
        ("unknown-target", "src/orphan.rs"),
        ("lib-bin-mismatch", "src/bin_only.rs"),
    ] {
        let ws = Workspace::new(&[
            ("Cargo.toml", MANIFEST),
            ("src/lib.rs", "pub fn api() {}\n"),
            ("src/main.rs", "mod bin_only;\nfn main() {}\n"),
            ("src/bin_only.rs", ""),
            ("src/orphan.rs", ""),
        ]);
        let record = item(&ws, "src/lib.rs", target, "api", 1, 1).await;
        assert!(record["returned"].get("error").is_some(), "{record}");
        assert_eq!(record["before"], record["after"]);
        save_named(id, record);
    }
}

#[tokio::test]
async fn module_lexical_scopes_and_literal_bytes() {
    let body = "pub fn f() -> u8 { 2 }\npub const RAW: &str = r###\"super :: f\n// super::f\"###;\npub const BYTES: &[u8] = b\"super::f\";\n/* super::f /* nested */ */\npub fn outer() -> u8 { super :: f() }\npub mod inner { pub fn value() -> u8 { super::f() + super :: super :: f() } }\n";
    let body = body.replace(
        "super :: super :: f()",
        "super /* first */ :: super /* second */ :: f()",
    );
    let lib = "pub mod a;\npub mod c;\n#[test] fn semantics() { assert_eq!(c::b::outer(), 1); assert_eq!(c::b::inner::value(), 3); assert_eq!(c::b::BYTES, b\"super::f\"); assert_eq!(c::b::RAW, \"super :: f\\n// super::f\"); }\n";
    let ws = Workspace::new(&[
        ("Cargo.toml", MANIFEST),
        ("src/lib.rs", lib),
        ("src/a.rs", "pub mod b;\npub fn f() -> u8 { 1 }\n"),
        ("src/a/b.rs", &body),
        ("src/c.rs", ""),
    ]);
    let record = module(&ws, "src/a/b.rs", "src/c/b.rs").await;
    let moved = ws.read("src/c/b.rs");
    assert!(moved.contains("super::f() + crate::a"), "{moved}");
    assert!(moved.contains("/* super::f /* nested */ */"));
    assert!(moved.contains("/* first */") && moved.contains("/* second */"));
    finish("lexical-super-scopes", &ws, record);
}

#[cfg(unix)]
#[tokio::test]
async fn directory_file_cycle_links_and_shared_targets_are_atomic() {
    for (id, relative, link_target) in [
        ("directory-link", "alias", "../../shared"),
        ("file-link", "alias.rs", "../../shared/mod.rs"),
        ("cycle-link", "cycle", "."),
    ] {
        let ws = Workspace::new(&[
            ("Cargo.toml", MANIFEST),
            ("src/lib.rs", "pub mod a;\npub mod c;\npub mod shared;\n"),
            ("src/a.rs", "pub mod b;\n"),
            ("src/a/b.rs", ""),
            ("src/c.rs", ""),
            ("src/shared/mod.rs", "pub const VALUE: u8 = 7;\n"),
        ]);
        let dir = ws.path("src/a/b");
        std::fs::create_dir_all(&dir).unwrap();
        let link = dir.join(relative);
        std::os::unix::fs::symlink(link_target, &link).unwrap();
        let identity = std::fs::canonicalize(ws.path("src/shared/mod.rs")).unwrap();
        let record = module(&ws, "src/a/b.rs", "src/c/b.rs").await;
        assert!(record["returned"].get("error").is_some(), "{record}");
        assert_eq!(record["before"], record["after"]);
        assert_eq!(std::fs::read_link(&link).unwrap(), Path::new(link_target));
        assert_eq!(
            std::fs::canonicalize(ws.path("src/shared/mod.rs")).unwrap(),
            identity
        );
        assert_eq!(ws.read("src/shared/mod.rs"), "pub const VALUE: u8 = 7;\n");
        save_named(id, record);
    }
}
