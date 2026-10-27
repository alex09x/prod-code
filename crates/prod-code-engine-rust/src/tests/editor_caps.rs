/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! #1516: an editor that did not advertise snippets, `documentChanges` or file operations in
//! `initialize` gets none of them back.

use crate::RustEngine;
use crate::editor::EditorCaps;
use serde_json::{Value, json};

const SUBJECT: &str = "pub struct Counter {
    pub total: u32,
}

impl Counter {
    pub fn add(&mut self, amount: u32) -> u32 {
        self.total += amount;
        self.total
    }
}

pub fn run() -> u32 {
    let mut counter = Counter { total: 0 };
    counter.add(2)
}

mod inner {
    pub fn helper() {}
}

pub fn other(c: &mut Counter) {
    c.
}
";

fn fixture() -> (tempfile::TempDir, RustEngine, String) {
    let temp = tempfile::tempdir().unwrap();
    super::configure_test_workspace(temp.path());
    std::fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname = \"subject\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(temp.path().join("src")).unwrap();
    let lib = temp.path().join("src/lib.rs");
    std::fs::write(&lib, SUBJECT).unwrap();
    let engine = RustEngine::load(temp.path()).expect("Must load fixture");
    let uri = crate::editor::lines::uri_of(&std::fs::canonicalize(&lib).unwrap());
    (temp, engine, uri)
}

fn method_item(completion: &Value) -> Value {
    completion["items"]
        .as_array()
        .and_then(|items| {
            items.iter().find(|i| {
                i["label"] == "add" || i["label"].as_str().is_some_and(|l| l.starts_with("add("))
            })
        })
        .cloned()
        .unwrap_or_else(|| panic!("add is offered: {completion}"))
}

#[test]
fn an_editor_without_snippets_or_document_changes_gets_neither() {
    let (_temp, engine, uri) = fixture();
    let bare = EditorCaps::from_initialize(&json!({ "capabilities": {} }));
    assert_eq!(
        bare,
        EditorCaps {
            snippets: false,
            document_changes: false,
            create_files: false,
            rename_files: false
        }
    );
    let at = json!({ "textDocument": { "uri": uri }, "position": { "line": 21, "character": 6 } });

    let full = engine
        .editor_request_with("textDocument/completion", &at, &EditorCaps::default())
        .unwrap()
        .unwrap();
    let add = method_item(&full);
    assert_eq!(
        add["insertTextFormat"], 2,
        "a capable editor gets the snippet: {add}"
    );

    let plain = engine
        .editor_request_with("textDocument/completion", &at, &bare)
        .unwrap()
        .unwrap();
    let add = method_item(&plain);
    assert_eq!(add["insertTextFormat"], 1, "{add}");
    assert!(!add.to_string().contains("${"), "no placeholder: {add}");

    // Moving `mod inner` to its own file creates one: refused without file operations, offered
    // as document changes with them.
    let actions = engine
        .editor_request_with(
            "textDocument/codeAction",
            &json!({
                "textDocument": { "uri": uri },
                "range": { "start": { "line": 16, "character": 4 }, "end": { "line": 16, "character": 4 } },
                "context": { "diagnostics": [] }
            }),
            &bare,
        )
        .unwrap()
        .unwrap();
    let to_file = actions
        .as_array()
        .and_then(|actions| {
            actions
                .iter()
                .find(|a| a["data"]["id"] == "move_module_to_file")
        })
        .cloned()
        .unwrap_or_else(|| panic!("the module can be moved to a file: {actions}"));
    let refused = engine
        .editor_request_with("codeAction/resolve", &to_file, &bare)
        .unwrap()
        .unwrap_err()
        .to_string();
    assert!(refused.contains("creates files"), "{refused}");
    let resolved = engine
        .editor_request_with("codeAction/resolve", &to_file, &EditorCaps::default())
        .unwrap()
        .unwrap();
    assert!(
        resolved["edit"]["documentChanges"]
            .as_array()
            .is_some_and(|c| c.iter().any(|c| c["kind"] == "create")),
        "{resolved}"
    );
}
