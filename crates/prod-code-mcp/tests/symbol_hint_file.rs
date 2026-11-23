/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A file given with a symbol to disambiguate is where the answer must be: a symbol the file
//! does not have is refused with where it was found, not answered with a same-named local of
//! an unrelated test (#2071).

use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use std::path::Path;

#[tokio::test]
async fn a_symbol_missing_from_the_given_file_is_refused_with_where_it_is() {
    let ws = Workspace::new(&[
        ("src/a.rs", "pub fn reconcile() {}\n"),
        (
            "tests/overlay.rs",
            "#[test]\nfn mapping() {\n    let saved_text = String::new();\n}\n",
        ),
    ]);
    let root = ws.root();
    let test_file = root.join("tests/overlay.rs");
    let t = test_file.clone();
    let gateway = ScriptedGateway::start(move |method, _params| match method {
        "workspace/symbol" => serde_json::json!([answers::symbol("saved_text", 13, &t, 3, 9)]),
        _ => serde_json::Value::Null,
    })
    .await;

    let err = prod_code_mcp::tools::resolve_symbol(
        gateway.addr(),
        &root,
        "saved_text",
        Some(Path::new("src/a.rs")),
    )
    .await
    .expect_err("src/a.rs has no saved_text");
    let text = format!("{err:#}");
    assert!(text.contains("no `saved_text` in src/a.rs"), "{text}");
    assert!(text.contains("overlay.rs"), "{text}");
    // The MCP tools pass the file as an absolute path; the answer names it from the root.
    let err = prod_code_mcp::tools::resolve_symbol(
        gateway.addr(),
        &root,
        "saved_text",
        Some(&root.join("src/a.rs")),
    )
    .await
    .expect_err("src/a.rs has no saved_text");
    assert!(
        format!("{err:#}").contains("no `saved_text` in src/a.rs"),
        "{err:#}"
    );

    // The same lookup with the file that has it, or with no file, still answers.
    for hint in [
        Some(Path::new("tests/overlay.rs")),
        Some(Path::new("./tests/overlay.rs")),
        None,
    ] {
        let hit = prod_code_mcp::tools::resolve_symbol(gateway.addr(), &root, "saved_text", hint)
            .await
            .unwrap_or_else(|e| panic!("{hint:?}: {e:#}"));
        assert_eq!(hit.path, test_file, "{hint:?}");
    }

    // A directory only says where to look.
    let hit = prod_code_mcp::tools::resolve_symbol(
        gateway.addr(),
        &root,
        "saved_text",
        Some(Path::new("tests")),
    )
    .await
    .expect("a directory does not refuse");
    assert_eq!(hit.path, test_file);
}
