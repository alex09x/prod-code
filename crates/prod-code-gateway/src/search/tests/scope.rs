/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::index::SearchIndexes;
use super::super::parser::declarations_in;
use super::super::runner::run_search;
use super::super::tokenizer::tokenize;
use super::fixtures::{Concepts, hit_files, scoped_request, write_scope_fixture};

#[test]
fn run_search_scopes_components_normalization_and_workspaces() {
    let storage = tempfile::tempdir().unwrap();
    let scope_root = storage.path().join("scope");
    write_scope_fixture(&scope_root);
    let other_root = storage.path().join("other");
    std::fs::create_dir_all(other_root.join("src/foo")).unwrap();
    std::fs::write(
        other_root.join("src/foo/other.rs"),
        "/// Scope target in another workspace.\npub fn other_workspace() {}\n",
    )
    .unwrap();
    let indexes = SearchIndexes::new();

    let scoped = run_search(
        &indexes,
        storage.path(),
        &scoped_request("scope", "scope target", Some("src/foo")),
    );
    assert_eq!(hit_files(&scoped), vec!["src/foo/nested.rs"]);

    let file = run_search(
        &indexes,
        storage.path(),
        &scoped_request("scope", "scope target", Some("src/foo.rs")),
    );
    assert_eq!(hit_files(&file), vec!["src/foo.rs"]);

    let unicode = run_search(
        &indexes,
        storage.path(),
        &scoped_request("scope", "scope target", Some("src/füß")),
    );
    assert_eq!(hit_files(&unicode), vec!["src/füß/δ.rs"]);

    let root = run_search(
        &indexes,
        storage.path(),
        &scoped_request("scope", "scope target", None),
    );
    for scope in [Some(""), Some("."), Some("./")] {
        let response = run_search(
            &indexes,
            storage.path(),
            &scoped_request("scope", "scope target", scope),
        );
        assert_eq!(hit_files(&response), hit_files(&root), "{scope:?}");
    }
    for scope in [Some("src/foo/"), Some("src/./foo"), Some("src\\.\\foo\\")] {
        let response = run_search(
            &indexes,
            storage.path(),
            &scoped_request("scope", "scope target", scope),
        );
        assert_eq!(hit_files(&response), vec!["src/foo/nested.rs"], "{scope:?}");
    }

    for scope in [
        "/src/foo",
        "../src/foo",
        "src/../foo",
        "C:\\src\\foo",
        "\\\\server\\share",
    ] {
        let response = run_search(
            &indexes,
            storage.path(),
            &scoped_request("scope", "scope target", Some(scope)),
        );
        assert!(
            response
                .error
                .as_deref()
                .is_some_and(|error| error.contains("invalid search subpath")),
            "{scope:?}: {response:?}"
        );
    }

    let missing = run_search(
        &indexes,
        storage.path(),
        &scoped_request("scope", "scope target", Some("src/missing")),
    );
    assert!(missing.error.is_none());
    assert!(missing.hits.is_empty());

    let other = run_search(
        &indexes,
        storage.path(),
        &scoped_request("other", "scope target", Some("src/foo")),
    );
    assert_eq!(hit_files(&other), vec!["src/foo/other.rs"]);
}

#[test]
fn run_search_scope_filters_dense_candidates_after_invalidation() {
    let storage = tempfile::tempdir().unwrap();
    let root = storage.path().join("dense");
    for (rel, source) in [
        (
            "src/foo/dense.rs",
            "/// Re-establishes the socket after a drop.\npub fn inside_scope() {}\n",
        ),
        (
            "src/foobar.rs",
            "/// Re-establishes the socket outside the requested directory.\npub fn sibling_scope() {}\n",
        ),
    ] {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, source).unwrap();
    }
    let indexes = SearchIndexes::with_embedders(Box::new(Concepts), Box::new(Concepts));
    let request = scoped_request("dense", "restore the connection", Some("src/foo"));
    let first = run_search(&indexes, storage.path(), &request);
    assert!(first.hits.is_empty(), "the lexical half shares no words");
    indexes.embed_pending(&root, 100);
    let dense = run_search(&indexes, storage.path(), &request);
    assert!(dense.dense.as_ref().is_some_and(|status| status.used));
    assert_eq!(hit_files(&dense), vec!["src/foo/dense.rs"]);

    std::fs::write(
        root.join("src/foo/dense.rs"),
        "/// Re-establishes the socket after a drop again.\npub fn inside_scope() {}\n",
    )
    .unwrap();
    indexes.invalidate(&root, ["src/foo/dense.rs"]);
    let _ = run_search(&indexes, storage.path(), &request);
    indexes.embed_pending(&root, 100);
    let refreshed = run_search(&indexes, storage.path(), &request);
    assert!(refreshed.dense.as_ref().is_some_and(|status| status.used));
    assert_eq!(hit_files(&refreshed), vec!["src/foo/dense.rs"]);
}

#[test]
fn tokenize_splits_humps_and_drops_stopwords() {
    assert_eq!(
        tokenize("where do we decide which node runs a workspace"),
        vec!["decide", "node", "run", "workspace"]
    );
    assert_eq!(
        tokenize("parseHTTPResponse"),
        vec!["parse", "http", "response"]
    );
    assert_eq!(
        tokenize("server_workspace_path"),
        vec!["server", "workspace", "path"]
    );
}

#[test]
fn rust_declarations_carry_their_doc_comment() {
    let src = "\
/// Decides which node should hold a workspace.
/// Falls back to the quietest live node.
pub async fn place(&self, name: &str) -> Option<String> {
    None
}

pub struct Metrics {
    pub count: u64,
}

impl Metrics {
    /// Records one event.
    pub fn record(&self, ev: Event) {}
}
";
    let decls = declarations_in("src/main.rs", src);
    let place = decls.iter().find(|d| d.name == "place").expect("place");
    assert_eq!(place.kind, "function");
    assert_eq!(place.line, 3);
    assert!(
        place
            .doc
            .starts_with("Decides which node should hold a workspace.")
    );
    assert!(place.signature.starts_with("pub async fn place"));
    let record = decls.iter().find(|d| d.name == "record").expect("record");
    assert_eq!(record.container.as_deref(), Some("Metrics"));
    assert_eq!(record.doc, "Records one event.");
    assert!(
        decls
            .iter()
            .any(|d| d.name == "Metrics" && d.kind == "struct")
    );
}

#[test]
fn other_languages_are_recognised() {
    let go = declarations_in(
        "pkg/x.go",
        "// Serve starts the listener.\nfunc Serve(addr string) error {\n}\n",
    );
    assert_eq!(go[0].name, "Serve");
    assert_eq!(go[0].doc, "Serve starts the listener.");
    let py = declarations_in(
        "app/x.py",
        "# Compute the signal.\ndef compute_signal(x):\n    pass\n",
    );
    assert_eq!(py[0].name, "compute_signal");
    assert_eq!(py[0].doc, "Compute the signal.");
    let ts = declarations_in(
        "src/x.ts",
        "/** Sends a frame. */\nexport function sendFrame(f: Frame) {}\n",
    );
    assert_eq!(ts[0].name, "sendFrame");
    assert!(ts[0].doc.contains("Sends a frame."));
    let swift = declarations_in(
        "Sources/x.swift",
        "/// Reloads the view.\npublic func reload() {}\n",
    );
    assert_eq!(swift[0].name, "reload");
}

/// C++ statements and calls are not functions, so they neither pollute the index nor use up
/// the per-file budget before a real definition (#1457).
#[test]
fn cpp_statements_and_calls_are_not_functions() {
    let mut src = String::from("int main() {\n");
    for _ in 0..450 {
        src.push_str("    if (ready) {\n    for (int i = 0; i < n; ++i) {\n    do_work(x);\n    y = compute(x);\n    } else if (b) {\n    return f(x);\n");
    }
    src.push_str("}\nvoid Engine::start(int port) {\n}\nWidget::Widget(int x) : x_(x) {\n}\nstatic int helper(const char *s);\n");
    let decls = declarations_in("src/engine.cpp", &src);
    let names: Vec<&str> = decls.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, ["main", "start", "Widget", "helper"], "{names:?}");
}

/// `#[cfg(test)]` marks only its own item, and a test module ends at its closing brace: the
/// production code after either is not test material (#1457).
#[test]
fn cfg_test_marks_only_its_item() {
    let src = "#[cfg(test)]\nuse crate::fixtures::sample;\n\npub fn production_after_import() {}\n\n#[cfg(test)]\nfn helper() {\n    let s = \"}\";\n}\n\npub fn production_after_helper() {}\n\n#[cfg(test)]\nmod tests {\n    fn inner() {\n        if x {\n        }\n    }\n    const FIXTURE: &str = r#\"\nfn fixture_decl() {\n}\n\"#;\n}\n\npub fn production_after_module() {}\n";
    let decls = declarations_in("src/lib.rs", src);
    let test = |name: &str| {
        decls
            .iter()
            .find(|d| d.name == name)
            .unwrap_or_else(|| panic!("{name} in {decls:?}"))
            .is_test
    };
    assert!(!test("production_after_import"));
    assert!(test("helper"));
    assert!(!test("production_after_helper"));
    assert!(test("inner"));
    assert!(test("fixture_decl"));
    assert!(!test("production_after_module"));
}

/// A search asked for more than the default pool returns as many hits as it asked for (#1458).
#[test]
fn a_limit_above_the_pool_is_honoured() {
    use super::super::graph::WorkspaceIndex;
    use super::super::scoring::rank;
    use super::super::types::{FileEntry, Indexed, POOL};
    let mut src = String::new();
    for n in 0..150 {
        src.push_str(&format!(
            "/// Opens widget number {n}.\npub fn open_widget_{n}() {{}}\n"
        ));
    }
    let mut index = WorkspaceIndex::default();
    let decls: Vec<Indexed> = declarations_in("src/widgets.rs", &src)
        .into_iter()
        .map(Indexed::new)
        .collect();
    assert_eq!(decls.len(), 150);
    index.files.insert(
        "src/widgets.rs".to_string(),
        FileEntry {
            stamp: (0, 0),
            generation: 0,
            decls,
        },
    );
    const { assert!(POOL < 100) };
    assert_eq!(rank(&index, "open widget", 100, None).len(), 100);
    assert_eq!(rank(&index, "open widget", 10, None).len(), 10);
}

/// A limit above what one search returns is refused with an error naming the bound, instead of
/// a silently shorter answer (#1458).
#[test]
fn a_limit_above_the_maximum_is_refused_by_name() {
    use super::super::types::MAX_LIMIT;
    let storage = tempfile::tempdir().unwrap();
    write_scope_fixture(&storage.path().join("scope"));
    let indexes = SearchIndexes::new();
    let mut request = scoped_request("scope", "scope target", None);
    request.limit = MAX_LIMIT + 1;
    let refused = run_search(&indexes, storage.path(), &request);
    assert!(refused.hits.is_empty());
    let error = refused.error.expect("an error");
    assert!(error.contains(&MAX_LIMIT.to_string()), "{error}");
    request.limit = MAX_LIMIT;
    assert_eq!(run_search(&indexes, storage.path(), &request).error, None);
}

/// On Unix `a:module.rs` is an ordinary file name: it is indexed, and its edit and deletion
/// reach the index instead of leaving stale declarations (#1459).
#[cfg(unix)]
#[test]
fn a_drive_like_unix_file_name_is_reindexed_and_removed() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let file = root.join("a:module.rs");
    std::fs::write(
        &file,
        "/// Parses the first ledger.\npub fn ledger_first() {}\n",
    )
    .unwrap();
    let indexes = SearchIndexes::new();
    let names = |query: &str| -> Vec<String> {
        indexes
            .search(&root, query, 5, None)
            .hits
            .into_iter()
            .map(|h| h.name)
            .collect()
    };
    assert_eq!(names("ledger"), ["ledger_first"]);

    std::fs::write(
        &file,
        "/// Parses the second ledger.\npub fn ledger_second() {}\n",
    )
    .unwrap();
    indexes.invalidate(&root, ["a:module.rs"]);
    assert_eq!(names("ledger"), ["ledger_second"], "the edit is reindexed");

    std::fs::remove_file(&file).unwrap();
    indexes.invalidate(&root, ["a:module.rs"]);
    assert!(names("ledger").is_empty(), "the deletion is reindexed");
}
