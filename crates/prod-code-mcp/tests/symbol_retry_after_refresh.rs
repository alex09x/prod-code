/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! An old session that just synced changed files may still be indexing them (#1422): an empty
//! `workspace/symbol` answer right after that refresh is asked again, as it is for a fresh
//! engine, however long ago the engine was loaded.

use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[tokio::test]
async fn an_empty_answer_right_after_a_refresh_is_asked_again() {
    let ws = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"x\"\nversion = \"0.1.0\"\n",
        ),
        ("src/lib.rs", "pub fn old() {}\n"),
    ]);
    let root = ws.root();
    let (lib, fresh) = (ws.path("src/lib.rs"), ws.path("src/fresh.rs"));
    let asked_fresh = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&asked_fresh);
    let (l, f) = (lib.clone(), fresh.clone());
    let gateway = ScriptedGateway::start(move |method, params| match method {
        // A non-gated engine loaded an hour ago.
        "prod-code/handshake" => serde_json::json!({ "engine_age_ms": 3_600_000u64 }),
        "workspace/symbol" => match params["query"].as_str() {
            Some("old") => serde_json::json!([answers::symbol("old", 12, &l, 1, 8)]),
            // The server is still indexing the file the refresh just synced.
            Some("fresh") if counter.fetch_add(1, Ordering::SeqCst) == 0 => serde_json::json!([]),
            Some("fresh") => serde_json::json!([answers::symbol("fresh", 12, &f, 1, 8)]),
            _ => serde_json::json!([]),
        },
        _ => serde_json::Value::Null,
    })
    .await;
    // The session is opened and pooled by a first lookup.
    let old = prod_code_mcp::tools::resolve_symbol(gateway.addr(), &root, "old", None)
        .await
        .expect("the old function");
    assert_eq!(old.path, lib);

    ws.write("src/fresh.rs", "pub fn fresh() {}\n");
    let hit = prod_code_mcp::tools::resolve_symbol(gateway.addr(), &root, "fresh", None)
        .await
        .expect("the new function, after the index settles");
    assert_eq!(hit.path, fresh);
    assert!(asked_fresh.load(Ordering::SeqCst) >= 2);
}
