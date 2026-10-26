/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#[path = "compile_check_projects/conditional.rs"]
mod conditional;
#[path = "compile_check_projects/consumers.rs"]
mod consumers;
#[path = "compile_check_projects/gateway.rs"]
mod gateway;
#[path = "compile_check_projects/multi_group.rs"]
mod multi_group;
#[path = "compile_check_projects/ownership.rs"]
mod ownership;
#[path = "compile_check_projects/review_cases.rs"]
mod review_cases;

use gateway::{CargoCompileGateway, assert_workspace_clean};
use prod_code_mcp::compile_check::check;
use prod_code_mcp::protocol::{McpContentItem, McpToolCallResult};
use prod_code_testkit::Workspace;

pub const ROOT_WORKSPACE_MANIFEST: &str = "\
[workspace]
members = [\"crates/member\"]
exclude = [\"crates/excluded\"]
resolver = \"3\"
";

pub const MEMBER_CARGO: &str =
    "[package]\nname = \"member\"\nversion = \"0.1.0\"\nedition = \"2024\"\n";
pub const MEMBER_LIB: &str = "pub fn member_value() -> u32 { 10 }\n";

pub const EXCLUDED_CARGO: &str =
    "[package]\nname = \"excluded\"\nversion = \"0.1.0\"\nedition = \"2024\"\n";
pub const EXCLUDED_LIB: &str = "pub fn excluded_value() -> u32 { 20 }\n";

const WORKSPACE_WITH_EXCLUDED: &[(&str, &str)] = &[
    ("Cargo.toml", ROOT_WORKSPACE_MANIFEST),
    ("crates/member/Cargo.toml", MEMBER_CARGO),
    ("crates/member/src/lib.rs", MEMBER_LIB),
    ("crates/excluded/Cargo.toml", EXCLUDED_CARGO),
    ("crates/excluded/src/lib.rs", EXCLUDED_LIB),
];

pub fn render_tool_text(result: &McpToolCallResult) -> String {
    result
        .content
        .iter()
        .map(|item| match item {
            McpContentItem::Text { text } => text.as_str(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn excluded_nested_crate_compiler_error_produces_failed_verdict() {
    let workspace = Workspace::new(WORKSPACE_WITH_EXCLUDED);
    let gateway = CargoCompileGateway::with_files(WORKSPACE_WITH_EXCLUDED).await;

    // 1. A proposed compiler error solely in the excluded crate must produce a failed verdict.
    let broken_files = vec![(
        workspace
            .path("crates/excluded/src/lib.rs")
            .to_string_lossy()
            .into_owned(),
        "pub fn excluded_value() -> u32 { \"not_an_integer\" }\n".to_string(),
    )];
    let broken_verdict = check(gateway.addr, &workspace.root(), &broken_files)
        .await
        .expect("compile_check::check runs on excluded crate");
    assert!(
        !broken_verdict.passed,
        "compiler error in excluded crate must fail verdict: {broken_verdict:?}"
    );
    assert!(
        broken_verdict
            .errors
            .iter()
            .any(|e| e.contains("crates/excluded/src/lib.rs") && e.contains("E0308")),
        "expected positioned E0308 error for excluded crate, got: {:?}",
        broken_verdict.errors
    );
    let rendered = broken_verdict.render();
    assert!(
        rendered.contains("the compiler rejects the result"),
        "{rendered}"
    );
    assert!(rendered.contains("E0308"), "{rendered}");

    let runs = gateway.take_runs();
    assert_eq!(
        runs.len(),
        1,
        "exactly one Cargo command for the excluded crate"
    );
    assert_eq!(runs[0].request.subdir.as_deref(), Some("crates/excluded"));
    assert_eq!(runs[0].exit_code, Some(101));
    assert!(runs[0].compiler_output.contains("E0308"));
    assert_eq!(runs[0].request.hypotheses[0].files.len(), 1);
    assert_eq!(
        runs[0].request.hypotheses[0].files[0].relative_path,
        "crates/excluded/src/lib.rs"
    );

    // 2. A valid edit in the excluded crate passes when checked in `crates/excluded`.
    let valid_files = vec![(
        "crates/excluded/src/lib.rs".to_string(),
        "pub fn excluded_value() -> u32 { 99 }\n".to_string(),
    )];
    let ok_verdict = check(gateway.addr, &workspace.root(), &valid_files)
        .await
        .expect("valid edit in excluded crate runs");
    assert!(ok_verdict.passed, "{ok_verdict:?}");
    assert!(ok_verdict.errors.is_empty(), "{ok_verdict:?}");
    let ok_runs = gateway.take_runs();
    assert_eq!(ok_runs.len(), 1);
    assert_eq!(
        ok_runs[0].request.subdir.as_deref(),
        Some("crates/excluded")
    );
    assert_eq!(ok_runs[0].exit_code, Some(0));

    assert_workspace_clean(
        &workspace,
        WORKSPACE_WITH_EXCLUDED,
        &[
            "crates/member/target",
            "crates/member/Cargo.lock",
            "crates/excluded/target",
            "crates/excluded/Cargo.lock",
        ],
    );
}

const GLOB_AND_IMPLICIT_WORKSPACE: &[(&str, &str)] = &[
    (
        "Cargo.toml",
        "[package]\nname = \"root_pkg\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[workspace]\nmembers = [\"crates/*\"]\nexclude = [\"crates/excluded\"]\nresolver = \"3\"\n[dependencies]\nimplicit_dep = { path = \"deps/implicit_dep\" }\n",
    ),
    (
        "src/lib.rs",
        "pub fn root_val() -> u32 { implicit_dep::dep_val() }\n",
    ),
    ("crates/member/Cargo.toml", MEMBER_CARGO),
    ("crates/member/src/lib.rs", MEMBER_LIB),
    ("crates/excluded/Cargo.toml", EXCLUDED_CARGO),
    ("crates/excluded/src/lib.rs", EXCLUDED_LIB),
    (
        "deps/implicit_dep/Cargo.toml",
        "[package]\nname = \"implicit_dep\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    ),
    (
        "deps/implicit_dep/src/lib.rs",
        "pub fn dep_val() -> u32 { 5 }\n",
    ),
];

const TOP_LEVEL_PLAIN_WITH_NESTED: &[(&str, &str)] = &[
    (
        "Cargo.toml",
        "[package]\nname = \"solo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    ),
    ("src/lib.rs", "pub fn solo() -> u32 { 1 }\n"),
    (
        "nested/Cargo.toml",
        "[package]\nname = \"nested\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    ),
    ("nested/src/lib.rs", "pub fn nested() -> u32 { 2 }\n"),
];

#[tokio::test]
async fn ordinary_member_package_and_top_level_behavior_preserved() {
    // 1. Explicit, glob, and implicit path-dependency workspace members all check at `subdir: None`.
    let workspace = Workspace::new(GLOB_AND_IMPLICIT_WORKSPACE);
    let gateway = CargoCompileGateway::with_files(GLOB_AND_IMPLICIT_WORKSPACE).await;

    for member_path in ["crates/member/src/lib.rs", "deps/implicit_dep/src/lib.rs"] {
        let broken = vec![(
            member_path.to_string(),
            "pub fn dep_val() -> u32 { missing_fn() }\n".to_string(),
        )];
        let verdict = check(gateway.addr, &workspace.root(), &broken)
            .await
            .unwrap();
        assert!(!verdict.passed, "{member_path}: {verdict:?}");
        assert!(
            verdict
                .errors
                .iter()
                .any(|e| e.contains(member_path) && e.contains("E0425")),
            "{member_path}: {:?}",
            verdict.errors
        );
        let runs = gateway.take_runs();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].request.subdir, None);
    }
    assert_workspace_clean(
        &workspace,
        GLOB_AND_IMPLICIT_WORKSPACE,
        &["crates/member/target", "deps/implicit_dep/target"],
    );

    // 2. Ordinary top-level package without `[workspace]` checks at `subdir: None`, while an
    //    unowned nested crate beneath it checks at `subdir: Some("nested")`.
    let plain_ws = Workspace::new(TOP_LEVEL_PLAIN_WITH_NESTED);
    let plain_gw = CargoCompileGateway::with_files(TOP_LEVEL_PLAIN_WITH_NESTED).await;

    let top_ok = check(
        plain_gw.addr,
        &plain_ws.root(),
        &[("src/lib.rs".into(), "pub fn solo() -> u32 { 42 }\n".into())],
    )
    .await
    .unwrap();
    assert!(top_ok.passed);
    let top_runs = plain_gw.take_runs();
    assert_eq!(top_runs.len(), 1);
    assert_eq!(top_runs[0].request.subdir, None);

    let nested_broken = check(
        plain_gw.addr,
        &plain_ws.root(),
        &[(
            "nested/src/lib.rs".into(),
            "pub fn nested() -> u32 { \"bad\" }\n".into(),
        )],
    )
    .await
    .unwrap();
    assert!(!nested_broken.passed);
    assert!(
        nested_broken
            .errors
            .iter()
            .any(|e| e.contains("nested/src/lib.rs") && e.contains("E0308")),
        "{:?}",
        nested_broken.errors
    );
    let nested_runs = plain_gw.take_runs();
    assert_eq!(nested_runs.len(), 1);
    assert_eq!(nested_runs[0].request.subdir.as_deref(), Some("nested"));
    assert_workspace_clean(&plain_ws, TOP_LEVEL_PLAIN_WITH_NESTED, &["nested/target"]);
}
