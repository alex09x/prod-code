/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::PathBuf;

use crate::engine::RustEngine;
use crate::tests::configure_test_workspace;

const LIB: &str = "pub fn leaf() -> u32 {\n    1\n}\n\npub fn mid() -> u32 {\n    leaf() + leaf()\n}\n\npub fn top() -> u32 {\n    mid()\n}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n\n    #[test]\n    fn checks_mid() {\n        assert_eq!(mid(), 2);\n    }\n}\n\npub struct Widget;\n\nimpl Widget {\n    pub fn build() -> Widget {\n        Widget\n    }\n}\n";

fn fixture() -> (tempfile::TempDir, PathBuf, RustEngine) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    configure_test_workspace(&root);
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    let lib = root.join("src/lib.rs");
    std::fs::write(&lib, LIB).unwrap();
    let engine = RustEngine::load(&root).expect("fixture loads");
    (dir, lib, engine)
}

/// 1-based (line, col) of the first `needle` in the fixture, `skip` occurrences in.
fn at(needle: &str, skip: usize) -> (u32, u32) {
    let off = LIB.match_indices(needle).nth(skip).unwrap().0;
    let line = LIB[..off].matches('\n').count() as u32 + 1;
    let col = (off - LIB[..off].rfind('\n').map_or(0, |i| i + 1)) as u32 + 1;
    (line, col)
}

fn names(items: &[crate::types::CallEdge]) -> Vec<(String, bool)> {
    let mut v: Vec<_> = items
        .iter()
        .map(|e| (e.item.name.clone(), e.is_test))
        .collect();
    v.sort();
    v
}

#[test]
fn prepare_resolves_the_enclosing_or_referenced_function() {
    let (_d, lib, engine) = fixture();
    let (l, c) = at("fn leaf", 0);
    let items = engine.prepare_call_hierarchy(&lib, l, c + 3).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "leaf");
    assert_eq!(items[0].kind, "Function");
    assert_eq!(
        (items[0].path.as_path(), items[0].line, items[0].col),
        (lib.as_path(), 1, 8)
    );
    assert_eq!(items[0].end_line, 3);
    // Inside a body, the enclosing function answers.
    let (l, c) = at("leaf() + leaf()", 0);
    let items = engine.prepare_call_hierarchy(&lib, l, c).unwrap();
    assert_eq!(
        items.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(),
        ["leaf"]
    );
    // A blank line has no function.
    assert!(
        engine
            .prepare_call_hierarchy(&lib, 4, 1)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn incoming_calls_name_callers_call_sites_and_mark_tests() {
    let (_d, lib, engine) = fixture();
    let (l, c) = at("fn leaf", 0);
    let edges = engine.incoming_calls(&lib, l, c + 3).unwrap();
    assert_eq!(names(&edges), [("mid".to_string(), false)]);
    let (l1, c1) = at("leaf() + leaf()", 0);
    assert_eq!(edges[0].call_sites, vec![(l1, c1), (l1, c1 + 9)]);
    let (l, c) = at("fn mid", 0);
    let edges = engine.incoming_calls(&lib, l, c + 3).unwrap();
    assert_eq!(
        names(&edges),
        [("checks_mid".to_string(), true), ("top".to_string(), false)]
    );
}

#[test]
fn outgoing_calls_list_callees_with_their_call_sites() {
    let (_d, lib, engine) = fixture();
    let (l, c) = at("fn mid", 0);
    let edges = engine.outgoing_calls(&lib, l, c + 3).unwrap();
    assert_eq!(names(&edges), [("leaf".to_string(), false)]);
    assert_eq!(edges[0].call_sites.len(), 2);
    assert_eq!(edges[0].item.line, 1);
    let (l, c) = at("fn leaf", 0);
    assert!(engine.outgoing_calls(&lib, l, c + 3).unwrap().is_empty());
    assert!(engine.incoming_calls(&lib, 4, 1).unwrap().is_empty());
}

#[test]
fn workspace_symbols_rank_exact_matches_and_honour_the_limit() {
    let (_d, lib, engine) = fixture();
    let snap = engine.snapshot();
    let hits = snap.workspace_symbols("leaf", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(
        (hits[0].name.as_str(), hits[0].kind.as_str()),
        ("leaf", "Function")
    );
    assert_eq!(
        (
            hits[0].path.as_path(),
            hits[0].line,
            hits[0].col,
            hits[0].end_line
        ),
        (lib.as_path(), 1, 8, 3)
    );
    // Prefix of three or more letters: fuzzy hits are returned after the exact ones.
    let hits = snap.workspace_symbols("Wid", 10).unwrap();
    assert!(
        hits.iter()
            .any(|h| h.name == "Widget" && h.kind == "Struct")
    );
    // Limit 0 is raised to 1.
    assert_eq!(snap.workspace_symbols("mid", 0).unwrap().len(), 1);
    // `Type::name` finds an associated function by its container.
    let hits = snap.workspace_symbols("Widget::build", 10).unwrap();
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(
        (hits[0].name.as_str(), hits[0].kind.as_str()),
        ("build", "Function")
    );
    assert_eq!(hits[0].line, at("fn build", 0).0);
    // Nothing anywhere: empty.
    assert!(
        snap.workspace_symbols("zzz_nothing_here", 5)
            .unwrap()
            .is_empty()
    );
    assert!(
        snap.workspace_symbols("Nope::nothing_here", 5)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn library_symbols_carry_their_crate_module_path() {
    let (_d, _lib, engine) = fixture();
    let snap = engine.snapshot();
    let hits = snap.workspace_symbols("HashMap", 5).unwrap();
    assert!(!hits.is_empty());
    assert!(hits.iter().all(|h| h.container.is_some()), "{hits:?}");
    assert!(hits.iter().any(|h| h.name == "HashMap"));
}
