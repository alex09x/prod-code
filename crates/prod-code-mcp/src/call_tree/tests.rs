/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;
use std::sync::Mutex;
use std::time::Instant;

use super::cache::{
    CALL_CACHE, CallCacheEntry, PREPARE_CACHE, PrepareCacheEntry, clear_call_hierarchy_cache,
    clear_call_hierarchy_cache_for, normalize_root,
};
use super::types::{CallTree, Node, key_of, node_of};

static TEST_MUTEX: Mutex<()> = Mutex::new(());

fn node(name: &str, children: Vec<Node>, repeated: bool) -> Node {
    Node {
        name: name.into(),
        uri: format!("file:///w/{name}.rs"),
        line: 1,
        col: 4,
        sites: vec!["3:5".into()],
        children,
        repeated,
    }
}

#[test]
fn a_tree_is_indented_by_level_and_a_repeat_is_marked() {
    let tree = CallTree {
        name: "leaf".into(),
        incoming: true,
        depth: 3,
        nodes: vec![node(
            "mid",
            vec![node("top", vec![], false), node("leaf", vec![], true)],
            false,
        )],
        truncated: true,
    };
    assert_eq!(tree.count(), 3);
    assert_eq!(
        tree.render(),
        "`leaf`: 1 caller(s), 3 in all to depth 3\n  \
         • mid  file:///w/mid.rs:1:4  [call sites: 3:5]\n    \
         • top  file:///w/top.rs:1:4  [call sites: 3:5]\n    \
         • leaf  file:///w/leaf.rs:1:4  [call sites: 3:5]  (shown above)\n\
         … stopped at 300 functions; ask for less depth or start lower"
    );
    let empty = CallTree {
        name: "f".into(),
        incoming: false,
        depth: 1,
        nodes: vec![],
        truncated: false,
    };
    assert_eq!(empty.render(), "`f`: 0 callee(s) — no callees found.");
}

#[test]
fn an_edge_gives_its_name_position_and_call_sites() {
    let other = serde_json::json!({
        "name": "caller",
        "uri": "file:///w/a.rs",
        "selectionRange": { "start": { "line": 9, "character": 3 }, "end": { "line": 9, "character": 9 } }
    });
    let edge = serde_json::json!({
        "from": other,
        "fromRanges": [
            { "start": { "line": 11, "character": 4 }, "end": { "line": 11, "character": 8 } },
            { "start": { "line": 12, "character": 0 }, "end": { "line": 12, "character": 4 } }
        ]
    });
    let node = node_of(&edge, &other);
    assert_eq!((node.name.as_str(), node.line, node.col), ("caller", 10, 4));
    assert_eq!(node.sites, vec!["12:5", "13:1"]);
    assert_eq!(key_of(&other), ("file:///w/a.rs".into(), 10, 4));
    assert_eq!(
        node_of(&serde_json::json!({}), &serde_json::json!({})).name,
        "?"
    );
}

#[test]
fn clear_call_hierarchy_cache_clears_entries() {
    let _guard = TEST_MUTEX.lock().unwrap();
    clear_call_hierarchy_cache();
    {
        let mut lock = CALL_CACHE.lock().unwrap();
        lock.insert(
            (
                "127.0.0.1:9000".parse().unwrap(),
                "/w".into(),
                "file:///w/a.rs".into(),
                1,
                1,
                true,
                1,
            ),
            CallCacheEntry {
                edges: serde_json::json!([]),
                timestamp: Instant::now(),
            },
        );
        assert_eq!(lock.len(), 1);
    }
    clear_call_hierarchy_cache();
    let lock = CALL_CACHE.lock().unwrap();
    assert!(lock.is_empty());
}

#[test]
fn clear_call_hierarchy_cache_for_retains_unrelated_roots() {
    let _guard = TEST_MUTEX.lock().unwrap();
    clear_call_hierarchy_cache();
    {
        let mut lock = CALL_CACHE.lock().unwrap();
        lock.insert(
            (
                "127.0.0.1:9000".parse().unwrap(),
                "/w1".into(),
                "file:///w1/a.rs".into(),
                1,
                1,
                true,
                1,
            ),
            CallCacheEntry {
                edges: serde_json::json!([]),
                timestamp: Instant::now(),
            },
        );
        lock.insert(
            (
                "127.0.0.1:9000".parse().unwrap(),
                "/w2".into(),
                "file:///w2/a.rs".into(),
                1,
                1,
                true,
                1,
            ),
            CallCacheEntry {
                edges: serde_json::json!([]),
                timestamp: Instant::now(),
            },
        );
        assert_eq!(lock.len(), 2);
    }
    clear_call_hierarchy_cache_for(Path::new("/w1"));
    {
        let lock = CALL_CACHE.lock().unwrap();
        assert_eq!(lock.len(), 1);
        assert!(lock.keys().any(|(_, r, ..)| r == "/w2"));
    }
    clear_call_hierarchy_cache();
}

#[test]
fn workspace_generation_invalidates_cached_graph() {
    let _guard = TEST_MUTEX.lock().unwrap();
    clear_call_hierarchy_cache();
    let remote = "127.0.0.1:9000".parse().unwrap();
    let root = Path::new("/test_workspace");
    let root_str = normalize_root(root);
    let uri = "file:///test_workspace/lib.rs";
    {
        let mut lock = PREPARE_CACHE.lock().unwrap();
        lock.insert(
            (remote, root_str.clone(), uri.into(), 10, 5, 1),
            PrepareCacheEntry {
                items: serde_json::json!([{ "name": "fn1", "uri": uri, "range": { "start": { "line": 9, "character": 0 }, "end": { "line": 9, "character": 10 } }, "selectionRange": { "start": { "line": 9, "character": 4 }, "end": { "line": 9, "character": 7 } } }]),
                timestamp: Instant::now(),
            },
        );
    }
    // Generation 2 cannot hit generation 1 entry
    let cached = {
        let lock = PREPARE_CACHE.lock().unwrap();
        lock.get(&(remote, root_str, uri.into(), 10, 5, 2)).cloned()
    };
    assert!(
        cached.is_none(),
        "workspace generation advance must invalidate cached item lookup"
    );
    clear_call_hierarchy_cache();
}
