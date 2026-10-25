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
use prod_code_testkit::Workspace;

async fn warns(name: &str, path: &str, before: &str, after: Option<&str>) -> bool {
    let mut warned = true;
    for poly in [false, true] {
        let ws = Workspace::new(&[(path, before)]);
        let g = gateway(vec![delta(path, after)], other_os()).await;
        let root = ws.root();
        let client = run(g.address, &root, poly, &["fixture"]);
        let release = async {
            g.arrived.await.unwrap();
            g.release.send(()).unwrap();
        };
        let (out, ()) = tokio::join!(client, release);
        g.task.await.unwrap();
        let warning = out.platform_warning(&root);
        println!(
            "{name} poly={poly}: before={before:?} after={after:?} relaid={:?} warning={warning:?}",
            out.relaid_files
        );
        warned &= warning.is_some();
        assert_eq!(
            std::fs::read_to_string(ws.path(path)).ok().as_deref(),
            after
        );
    }
    warned
}

#[tokio::test]
async fn cfg_macro_and_nested_predicates_warn() {
    let mut failures = vec![];
    for text in [
        "fn f() { if cfg!(target_os = \"macos\") { old(); } }",
        "#[cfg(not(unix))] fn f() { old(); }",
        "#[cfg(any(target_os = \"macos\", all(windows, not(unix))))] fn f() { old(); }",
    ] {
        if !warns("cfg", "file.rs", text, Some(&text.replace("old", "new"))).await {
            failures.push(text);
        }
    }
    assert!(failures.is_empty(), "missed predicates: {failures:?}");
}

#[tokio::test]
async fn deleted_or_rewritten_away_platform_evidence_warns() {
    let deleted = warns("deleted", "file.rs", "fn f() { libc::open(); }", None).await;
    let rewritten = warns(
        "rewritten",
        "file.rs",
        "fn f() { libc::open(); }",
        Some("fn f() {} "),
    )
    .await;
    assert!(
        deleted && rewritten,
        "lost pre-write evidence: deleted={deleted} rewritten={rewritten}"
    );
}

#[tokio::test]
async fn argument_reordering_and_tuple_comma_are_semantic() {
    let reordered = warns(
        "argument-order",
        "file.rs",
        "fn f() { libc::foo(a, b); }",
        Some("fn f() { libc::foo(b, a); }"),
    )
    .await;
    let tuple = warns(
        "tuple-comma",
        "file.rs",
        "fn f() { libc::foo((value)); }",
        Some("fn f() { libc::foo((value,)); }"),
    )
    .await;
    assert!(
        reordered && tuple,
        "semantic edit suppressed: reorder={reordered} tuple={tuple}"
    );
}

#[tokio::test]
async fn c_control_flow_brace_removal_is_semantic() {
    assert!(
        warns(
            "c-braces",
            "file.c",
            "#ifdef __APPLE__\nif (apple) { open(); cleanup(); }\n#endif",
            Some("#ifdef __APPLE__\nif (apple) open(); cleanup();\n#endif")
        )
        .await
    );
}
