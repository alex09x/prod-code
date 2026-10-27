/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A file of another checkout reached through a symlink alias is asked about by its real path
//! (#1423): the session opens on that checkout's real root, and the documents it opens and asks
//! about carry the real path too, which the node can translate into its copy.

use prod_code_testkit::{ScriptedGateway, Workspace};
use std::sync::{Arc, Mutex};

#[cfg(unix)]
#[tokio::test]
async fn a_symlinked_external_checkout_is_asked_by_its_real_paths() {
    let here = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"a\"\nversion = \"0.1.0\"\n",
        ),
        ("src/lib.rs", "pub fn a() {}\n"),
    ]);
    let other = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"b\"\nversion = \"0.1.0\"\n",
        ),
        ("src/lib.rs", "pub fn b() {}\n"),
    ]);
    let links = tempfile::tempdir().unwrap();
    let alias = links.path().join("b-alias");
    std::os::unix::fs::symlink(other.root(), &alias).unwrap();
    let real = std::fs::canonicalize(other.root().join("src/lib.rs")).unwrap();
    let aliased = alias.join("src/lib.rs");

    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    let gateway = ScriptedGateway::start(move |method, params| {
        if let Some(uri) = params.pointer("/textDocument/uri").and_then(|u| u.as_str()) {
            log.lock().unwrap().push(format!("{method} {uri}"));
        }
        match method {
            "textDocument/hover" => serde_json::json!({ "contents": "fn b()" }),
            _ => serde_json::Value::Null,
        }
    })
    .await;
    let params = serde_json::json!({
        "textDocument": { "uri": url::Url::from_file_path(&aliased).unwrap().to_string() },
        "position": { "line": 0, "character": 7 }
    });
    prod_code_mcp::session::pooled_query(
        gateway.addr(),
        &here.root(),
        &aliased,
        "textDocument/hover",
        params,
    )
    .await
    .expect("the hover");

    let real_uri = url::Url::from_file_path(&real).unwrap().to_string();
    let seen = seen.lock().unwrap().clone();
    assert!(
        seen.iter()
            .any(|s| s == &format!("textDocument/hover {real_uri}")),
        "{seen:?}"
    );
    assert!(
        !seen.iter().any(|s| s.contains("b-alias")),
        "no request carries the alias: {seen:?}"
    );
}
