/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Qualified definition lookup through MCP and the real Rust analyzer, without discovery,
//! admission based on machine load, or a separately installed gateway.

use prod_code_mcp::protocol::McpContentItem;
use prod_code_mcp::tools::execute_tool;
use serde_json::json;
use std::sync::Arc;

const FILES: &[(&str, &str)] = &[
    (
        "Cargo.toml",
        "[package]\nname = \"qualified_fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    ),
    ("prod-code.toml", "[rust]\nproc_macro_srv = \"disabled\"\n"),
    (
        "src/lib.rs",
        concat!(
            "pub mod pricing;\npub mod accounts;\npub mod fixtures;\npub mod left { pub mod shared {\n    pub fn repeat() {\n        let body_marker = 1;\n    }\n} }\npub mod right { pub mod shared {\n    pub fn repeat() {\n        let body_marker = 2;\n    }\n} }\npub fn noise(state: u32, order: u32) {\n    let state = state;\n    let order = order;\n}\n",
            "pub mod a;\npub mod b;\npub mod nested;\n",
        ),
    ),
    (
        "src/pricing.rs",
        "#[path = \"pricing_fallback.rs\"]\npub mod fallback;\npub fn limits() {\n    let cap = 1;\n}\n",
    ),
    (
        "src/pricing_fallback.rs",
        "pub fn cap() -> u32 {\n    42\n}\n",
    ),
    (
        "src/account_action.rs",
        "pub fn approver() -> u32 {\n    7\n}\n",
    ),
    (
        "src/accounts.rs",
        "#[path = \"account_action.rs\"]\npub mod account_action;\npub fn approvals() {\n    let approver = 1;\n    let approver = 2;\n    let approver = 3;\n    let approver = 4;\n    let approver = 5;\n    let approver = 6;\n    let approver = 7;\n    let approver = 8;\n}\n",
    ),
    (
        "src/fixtures.rs",
        "pub fn state() {}\npub fn order() {}\npub struct Noise { pub state: u32, pub order: u32 }\npub fn noise(state: u32, order: u32) {\n    let state = state;\n    let order = order;\n}\npub struct Type<T>(pub T);\nimpl<T> Type<T> {\n    pub fn method() {}\n}\npub trait Trait {\n    fn method();\n}\npub fn stateful() {}\nimpl<T> Type<T> {\n    pub fn locals() {\n        let local_only = 1;\n    }\n    pub fn parameters(\n        parameter_only: u32,\n    ) {}\n}\npub static LIMIT: u32 = 3;\npub const MAX_ITEMS: usize = 4;\n",
    ),
    (
        "src/a.rs",
        "pub fn name() {}\npub fn relative_name() {}\npub(crate) fn internal_name() {}\npub(crate) fn internal_relative_name() {}\npub struct Type;\nimpl Type {\n    pub fn name() { crate::a::name(); }\n}\n",
    ),
    (
        "src/b.rs",
        "use crate::a;\npub fn name() { crate::a::name(); }\npub fn relative_name() { a::relative_name(); }\npub fn internal_name() { crate::a::internal_name(); }\npub fn internal_relative_name() { a::internal_relative_name(); }\n",
    ),
    (
        "src/nested.rs",
        "pub mod b {\n    use crate::a;\n    pub fn name() { crate::a::name(); }\n    pub fn relative_name() { a::relative_name(); }\n    pub fn internal_name() { crate::a::internal_name(); }\n    pub fn internal_relative_name() { a::internal_relative_name(); }\n}\n",
    ),
];

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn qualified_definition_real_analyzer_table() {
    let storage = tempfile::tempdir().unwrap();
    let mut state = prod_code_gateway::ServerState::new(storage.path().to_path_buf());
    state.workspace_manager = Arc::new(
        prod_code_gateway::workspace::WorkspaceManager::with_admission(Arc::new(
            prod_code_gateway::admission::Admission::unbounded(),
        )),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let remote = listener.local_addr().unwrap();
    *state.advertise.write().await = remote.to_string();
    let state = Arc::new(state);
    let server = tokio::spawn(async move {
        let mut clients = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let (socket, peer) = accepted.unwrap();
                    let state = Arc::clone(&state);
                    clients.spawn(async move {
                        prod_code_gateway::handle_client(socket, peer, state).await
                    });
                }
                _ = clients.join_next(), if !clients.is_empty() => {}
            }
        }
    });
    let fixture = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(fixture.path()).unwrap();
    for (relative, source) in FILES {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, source).unwrap();
    }
    for args in [
        vec!["init", "-q"],
        vec!["add", "-A"],
        vec![
            "-c",
            "user.name=test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-qm",
            "fixture",
        ],
    ] {
        assert!(
            std::process::Command::new("git")
                .args(args)
                .current_dir(&root)
                .status()
                .unwrap()
                .success()
        );
    }

    // Load the real project before exercising the search fast paths' short query budgets.
    // The outline is also the independent control for the #[path] declaration's location.
    let outline = execute_tool(
        remote,
        &root,
        "code_outline",
        json!({ "path": "src/pricing_fallback.rs" }),
    )
    .await
    .unwrap();
    assert!(text(&outline).contains("cap"), "{}", text(&outline));
    for query in ["pricing::fallback::cap", "cap"] {
        let index = prod_code_mcp::tools::execute_lsp_query(
            remote,
            &root,
            &root.join("src/lib.rs"),
            "workspace/symbol",
            json!({ "query": query, "limit": 200 }),
        )
        .await
        .unwrap();
        let declarations: Vec<_> = index
            .as_array()
            .unwrap()
            .iter()
            .filter(|hit| hit["name"] == "cap")
            .collect();
        assert!(!declarations.is_empty(), "{query}: {index}");
        for declaration in declarations {
            assert_eq!(
                declaration["containerName"], "qualified_fixture::pricing::fallback",
                "{query}: {index}"
            );
            assert!(
                declaration["location"]["uri"]
                    .as_str()
                    .unwrap()
                    .ends_with("/src/pricing_fallback.rs"),
                "{query}: {index}"
            );
            assert_eq!(
                declaration["location"]["range"]["start"],
                json!({ "line": 0, "character": 7 }),
                "{query}: {index}"
            );
        }
    }

    let cases = [
        (
            "pricing::fallback::cap",
            None,
            "src/pricing_fallback.rs:1:8",
        ),
        (
            "crate::pricing::fallback::cap",
            None,
            "src/pricing_fallback.rs:1:8",
        ),
        (
            "qualified_fixture::pricing::fallback::cap",
            None,
            "src/pricing_fallback.rs:1:8",
        ),
        (
            "account_action::approver",
            None,
            "src/account_action.rs:1:8",
        ),
        (
            "crate::accounts::account_action::approver",
            None,
            "src/account_action.rs:1:8",
        ),
        ("fixtures::state", None, "src/fixtures.rs:1:8"),
        ("fixtures::order", None, "src/fixtures.rs:2:8"),
        ("fixtures::Type::method", None, "src/fixtures.rs:10:12"),
        ("fixtures::Trait::method", None, "src/fixtures.rs:13:8"),
        // A qualified static, const or field is a declaration, not a local binding.
        ("fixtures::LIMIT", None, "src/fixtures.rs:24:12"),
        ("fixtures::MAX_ITEMS", None, "src/fixtures.rs:25:11"),
        ("fixtures::Noise::state", None, "src/fixtures.rs:3:24"),
        ("Type::method", None, "src/fixtures.rs:10:12"),
        ("Trait::method", None, "src/fixtures.rs:13:8"),
        // Same-named wrappers must resolve to their own declarations in both directions,
        // regardless of the qualified call in their bodies or the original's visibility.
        ("a::name", None, "src/a.rs:1:8"),
        ("b::name", None, "src/b.rs:2:8"),
        ("nested::b::name", None, "src/nested.rs:3:12"),
        ("a::relative_name", None, "src/a.rs:2:8"),
        ("b::relative_name", None, "src/b.rs:3:8"),
        ("nested::b::relative_name", None, "src/nested.rs:4:12"),
        ("a::internal_name", None, "src/a.rs:3:15"),
        ("b::internal_name", None, "src/b.rs:4:8"),
        ("nested::b::internal_name", None, "src/nested.rs:5:12"),
        ("a::internal_relative_name", None, "src/a.rs:4:15"),
        ("b::internal_relative_name", None, "src/b.rs:5:8"),
        (
            "nested::b::internal_relative_name",
            None,
            "src/nested.rs:6:12",
        ),
        ("a::Type::name", None, "src/a.rs:7:12"),
        (
            "self::fallback::cap",
            Some("src/pricing.rs"),
            "src/pricing_fallback.rs:1:8",
        ),
        (
            "super::fallback::cap",
            Some("src/pricing_fallback.rs"),
            "src/pricing_fallback.rs:1:8",
        ),
    ];
    let mut failures = Vec::new();
    for (symbol, hint, expected) in cases {
        let mut args = json!({ "symbol": symbol });
        if let Some(hint) = hint {
            args["path"] = json!(hint);
        }
        match execute_tool(remote, &root, "code_definition", args).await {
            Ok(result) => {
                let text = text(&result);
                let definitions: Vec<_> = text
                    .lines()
                    .filter_map(|line| line.strip_prefix("📍 Definition: "))
                    .collect();
                let expected_location = format!(
                    "{}{expected}",
                    url::Url::from_directory_path(&root).unwrap()
                );
                if result.is_error || definitions != [expected_location.as_str()] {
                    failures.push(format!("{symbol}: expected only {expected}; got {text}"));
                }
            }
            Err(error) => failures.push(format!("{symbol}: expected {expected}; got {error:#}")),
        }
    }
    // Both declarations are equally eligible; preview them without expanding their bodies.
    match execute_tool(
        remote,
        &root,
        "code_definition",
        json!({ "symbol": "shared::repeat" }),
    )
    .await
    {
        Ok(result) => {
            let text = text(&result);
            if !text.contains("ambiguous (2 candidates)")
                || !text.contains("left::shared::repeat")
                || !text.contains("right::shared::repeat")
                || text.contains("body_marker")
                || text.contains("📍 Definition:")
                || text.lines().count() > 10
            {
                failures.push(format!(
                    "shared::repeat: expected short ambiguity; got {text}"
                ));
            }
        }
        Err(error) => {
            let text = format!("{error:#}");
            if !text.contains("ambiguous (2 candidates)")
                || !text.contains("left::shared::repeat")
                || !text.contains("right::shared::repeat")
                || text.contains("body_marker")
                || text.lines().count() > 10
            {
                failures.push(format!("shared::repeat: {text}"));
            }
        }
    }
    for (symbol, closest) in [
        ("fixtures::states", "fixtures::state"),
        ("fixtures::Type::state", "fixtures::state"),
        ("missing::state", "fixtures::state"),
        ("fixtures::Type::local_only", "Type<T>::locals"),
        ("fixtures::Type::parameter_only", "Type<T>::parameters"),
    ] {
        let outcome = execute_tool(
            remote,
            &root,
            "code_definition",
            json!({ "symbol": symbol }),
        )
        .await;
        let error = match outcome {
            Ok(result) => text(&result),
            Err(error) => format!("{error:#}"),
        };
        if !error.contains(&format!("no symbol named `{symbol}`"))
            || !error.contains(closest)
            || !error.contains("did you mean")
            || error.contains("📍 Definition:")
        {
            failures.push(format!(
                "{symbol}: expected not-found with closest declarations; got {error}"
            ));
        }
    }
    server.abort();
    let _ = server.await;
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

fn text(result: &prod_code_mcp::protocol::McpToolCallResult) -> String {
    result
        .content
        .iter()
        .map(|item| match item {
            McpContentItem::Text { text } => text.as_str(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}
