/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::*;
use prod_code_engine_rust::{FileMove, RefactorOutcome, RewrittenFile};

/// A refactoring's rewrites are read at the paths the files had before it, and LSP applies
/// `documentChanges` in order, so they go out before the moves. Sent after them, the rewrite
/// of `a.rs` would land on the file `c.rs` was just moved to. Applied by the client, a module
/// rename (`foo.rs` and `foo/`, with a file created inside it) lands whole.
#[test]
fn a_refactoring_is_serialized_with_its_rewrites_before_its_moves() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let write = |rel: &str, text: &str| {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    write("src/lib.rs", "mod foo;\nmod a;\nmod c;\n");
    write("src/foo.rs", "mod inner;\npub use inner::f;\n");
    write("src/foo/inner.rs", "pub fn f() -> u8 { crate::foo::X }\n");
    write("src/a.rs", "pub const A: u8 = 1;\n");
    write("src/c.rs", "pub const C: u8 = 3;\n");
    let rewrite = |rel: &str, new_text: &str| RewrittenFile {
        path: root.join(rel),
        new_text: new_text.to_string(),
        edits: 1,
        old_line_count: std::fs::read_to_string(root.join(rel))
            .unwrap()
            .lines()
            .count() as u32,
    };
    let moved = |from: &str, to: &str| FileMove {
        from: root.join(from),
        to: root.join(to),
    };
    let outcome = RefactorOutcome {
        files: vec![
            rewrite("src/lib.rs", "mod bar;\nmod b;\nmod a;\n"),
            rewrite("src/foo.rs", "mod inner;\nmod extra;\npub use inner::f;\n"),
            rewrite("src/foo/inner.rs", "pub fn f() -> u8 { crate::bar::X }\n"),
            rewrite("src/a.rs", "pub const B: u8 = 1;\n"),
            rewrite("src/c.rs", "pub const A: u8 = 3;\n"),
        ],
        created: vec![RewrittenFile {
            path: root.join("src/foo/extra.rs"),
            new_text: "pub fn extra() {}\n".to_string(),
            edits: 1,
            old_line_count: 0,
        }],
        moves: vec![
            moved("src/foo.rs", "src/bar.rs"),
            moved("src/foo", "src/bar"),
            moved("src/a.rs", "src/b.rs"),
            moved("src/c.rs", "src/a.rs"),
        ],
    };
    let edit = super::super::workspace_edit_json(&outcome);
    let kinds: Vec<&str> = edit["documentChanges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|change| change["kind"].as_str().unwrap_or("edit"))
        .collect();
    assert_eq!(
        kinds,
        [
            "create", "edit", "edit", "edit", "edit", "edit", "edit", "rename", "rename", "rename",
            "rename"
        ]
    );
    prod_code_mcp::refactor::apply_workspace_edit(&root, &edit).unwrap();
    let read = |rel: &str| std::fs::read_to_string(root.join(rel)).ok();
    assert_eq!(
        read("src/lib.rs").as_deref(),
        Some("mod bar;\nmod b;\nmod a;\n")
    );
    assert_eq!(
        read("src/bar.rs").as_deref(),
        Some("mod inner;\nmod extra;\npub use inner::f;\n")
    );
    assert_eq!(
        read("src/bar/inner.rs").as_deref(),
        Some("pub fn f() -> u8 { crate::bar::X }\n")
    );
    assert_eq!(
        read("src/bar/extra.rs").as_deref(),
        Some("pub fn extra() {}\n")
    );
    assert_eq!(read("src/b.rs").as_deref(), Some("pub const B: u8 = 1;\n"));
    assert_eq!(read("src/a.rs").as_deref(), Some("pub const A: u8 = 3;\n"));
    for gone in ["src/foo.rs", "src/foo", "src/c.rs"] {
        assert!(!root.join(gone).exists(), "{gone} was moved away");
    }
    prod_code_mcp::sync::clear_sync_cache(&root);
}

/// The same through rust-analyzer: renaming the module `foo`, kept in `foo.rs` with its
/// submodule in `foo/`, moves both and rewrites every use, the one inside `foo/` included,
/// and the client lands all of it.
#[test]
fn a_module_rename_by_the_analyzer_lands_whole_in_the_checkout() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let write = |rel: &str, text: &str| {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(
        "src/lib.rs",
        "pub mod foo;\npub fn g() -> u8 { foo::inner::f() }\n",
    );
    write("src/foo.rs", "pub mod inner;\npub const X: u8 = 1;\n");
    write("src/foo/inner.rs", "pub fn f() -> u8 { crate::foo::X }\n");
    let engine = prod_code_engine_rust::RustEngine::load(&root).unwrap();
    // `foo` in `pub mod foo;` is line 1, column 9.
    let outcome = engine
        .rename(&root.join("src/lib.rs"), 1, 9, "bar")
        .expect("rename query")
        .expect("rename accepted");
    assert_eq!(outcome.moves.len(), 2, "{outcome:?}");
    let edit = super::super::workspace_edit_json(&outcome);
    prod_code_mcp::refactor::apply_workspace_edit(&root, &edit).unwrap();
    let read = |rel: &str| std::fs::read_to_string(root.join(rel)).ok();
    assert_eq!(
        read("src/lib.rs").as_deref(),
        Some("pub mod bar;\npub fn g() -> u8 { bar::inner::f() }\n")
    );
    assert_eq!(
        read("src/bar.rs").as_deref(),
        Some("pub mod inner;\npub const X: u8 = 1;\n")
    );
    assert_eq!(
        read("src/bar/inner.rs").as_deref(),
        Some("pub fn f() -> u8 { crate::bar::X }\n")
    );
    assert!(!root.join("src/foo.rs").exists() && !root.join("src/foo").exists());
    prod_code_mcp::sync::clear_sync_cache(&root);
}

/// Renaming a test function inside `#[cfg(test)]` succeeds without hanging and lands whole.
#[tokio::test]
async fn a_rust_test_function_rename_lands_in_the_checkout() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let write = |rel: &str, text: &str| {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(
        "src/lib.rs",
        "#[cfg(test)]\nmod tests {\n    #[test]\n    fn test_initial_metric() {}\n}\n",
    );
    let engine = prod_code_engine_rust::RustEngine::load(&root).unwrap();
    let engine_lock = Arc::new(tokio::sync::Mutex::new(engine));

    let (raw_tx, mut rx) = rapidfire::mpsc::bounded(16);
    let out_tx = SharedOutputSender::new(raw_tx, Duration::from_secs(5));
    let root_str = root.to_string_lossy();
    let translator = PathTranslator::new(&root_str, &root_str);
    let workspace = Arc::new(crate::workspace::shared::SharedWorkspace::new(
        root.clone(),
        "rust".to_string(),
        Some(Arc::clone(&engine_lock)),
        None,
        None,
        None,
    ));
    let view = SessionView {
        session_id: 1,
        worktree_root: root.clone(),
        workspace: Arc::clone(&workspace),
        accounted: Arc::clone(&workspace),
        is_single_owner: Arc::new(std::sync::atomic::AtomicBool::new(true)),
        direct_edit_open_files: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        lease: None,
        owner: None,
    };

    // `test_initial_metric` is at line 4, col 8 (0-based LSP position: line 3, char 7).
    let params = serde_json::json!({
        "textDocument": { "uri": format!("file://{}", root.join("src/lib.rs").display()) },
        "position": { "line": 3, "character": 7 },
        "newName": "test_updated_metric"
    });

    lsp_rename(
        &out_tx,
        &translator,
        &view,
        &Some(serde_json::json!(100)),
        &params,
        &engine_lock,
    );

    let frame = tokio::time::timeout(Duration::from_secs(90), rx.recv())
        .await
        .expect("rename responded within timeout")
        .expect("channel yielded response");

    let WireMessage::LspPayload(payload) = frame.message else {
        panic!("expected WireMessage::LspPayload");
    };
    let val: serde_json::Value = serde_json::from_str(&payload).expect("valid LSP JSON");
    assert_eq!(val["id"], 100);
    assert!(val.get("error").is_none(), "LSP returned error: {val:?}");
    let result = &val["result"];
    prod_code_mcp::refactor::apply_workspace_edit(&root, result).unwrap();

    let read = std::fs::read_to_string(root.join("src/lib.rs")).unwrap();
    assert!(read.contains("fn test_updated_metric()"));
    assert!(!read.contains("fn test_initial_metric()"));

    // Verify engine lock is released and usable concurrently
    assert!(engine_lock.try_lock().is_ok());

    prod_code_mcp::sync::clear_sync_cache(&root);
}

#[cfg(test)]
mod analyzer_panic_tests {
    use super::*;

    #[test]
    fn a_panic_is_reported_as_one_unchecked_file_not_as_a_failed_request() {
        let payload: Box<dyn std::any::Any + Send> = Box::new("escaping bound vars.".to_string());
        let message = panic_message(payload);
        assert_eq!(message, "escaping bound vars.");
        assert_eq!(panic_message(Box::new("static text")), "static text");
        assert_eq!(panic_message(Box::new(42u8)), "no message");

        let report = analyzer_panic_report(&message);
        let items = report["items"].as_array().expect("items");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["severity"], 1);
        assert_eq!(items[0]["code"], ANALYZER_PANIC);
        assert_eq!(items[0]["range"]["start"]["line"], 0);
        let text = items[0]["message"].as_str().unwrap();
        assert!(text.contains("nothing in it was checked"), "{text}");
        assert!(
            text.contains("escaping bound vars. The compiler"),
            "one full stop: {text}"
        );
        assert!(text.contains("verify"), "{text}");
    }
}
