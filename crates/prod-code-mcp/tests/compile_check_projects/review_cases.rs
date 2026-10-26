/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::gateway::CargoCompileGateway;
use super::{MEMBER_CARGO, MEMBER_LIB, ROOT_WORKSPACE_MANIFEST};
use prod_code_mcp::compile_check::check;
use prod_code_testkit::Workspace;

const BAD: &str = "pub fn value() -> u32 { \"wrong\" }\n";
const GOOD: &str = "pub fn value() -> u32 { 1 }\n";

const EXTERNAL: &[(&str, &str)] = &[
    ("Cargo.toml", ROOT_WORKSPACE_MANIFEST),
    ("crates/member/Cargo.toml", MEMBER_CARGO),
    ("crates/member/src/lib.rs", MEMBER_LIB),
    (
        "crates/excluded/Cargo.toml",
        "[package]\nname = \"external\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[lib]\npath = \"../../shared/lib.rs\"\n",
    ),
    ("shared/lib.rs", GOOD),
];

const TARGET_EXTENSION: &[(&str, &str)] = &[
    ("Cargo.toml", ROOT_WORKSPACE_MANIFEST),
    ("crates/member/Cargo.toml", MEMBER_CARGO),
    ("crates/member/src/lib.rs", MEMBER_LIB),
    (
        "crates/excluded/Cargo.toml",
        "[package]\nname = \"external\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[lib]\npath = \"../../shared/source.code\"\n",
    ),
    ("shared/source.code", GOOD),
];

#[tokio::test]
async fn review_explicit_target_ownership_does_not_depend_on_extension() {
    let workspace = Workspace::new(TARGET_EXTENSION);
    let gateway = CargoCompileGateway::with_files(TARGET_EXTENSION).await;
    let verdict = check(
        gateway.addr,
        &workspace.root(),
        &[("shared/source.code".into(), BAD.into())],
    )
    .await
    .unwrap();
    eprintln!("target extension verdict: {verdict:?}");
    assert!(
        !verdict.passed,
        "Cargo target path establishes language: {verdict:?}"
    );
    let runs = gateway.take_runs();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].request.subdir.as_deref(), Some("crates/excluded"));
    assert_eq!(runs[0].exit_code, Some(101));
    assert!(
        verdict
            .errors
            .iter()
            .any(|e| e.starts_with("shared/source.code:"))
    );
}

#[tokio::test]
async fn review_external_excluded_target() {
    let workspace = Workspace::new(EXTERNAL);
    let gateway = CargoCompileGateway::with_files(EXTERNAL).await;
    let verdict = check(
        gateway.addr,
        &workspace.root(),
        &[("shared/lib.rs".into(), BAD.into())],
    )
    .await
    .unwrap();
    eprintln!("external verdict: {verdict:?}");
    assert!(
        !verdict.passed,
        "external excluded target must compile: {verdict:?}"
    );
    let runs = gateway.take_runs();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].request.subdir.as_deref(), Some("crates/excluded"));
    assert_eq!(runs[0].exit_code, Some(101));
    assert_eq!(verdict.errors.len(), 1);
    assert!(verdict.errors[0].starts_with("shared/lib.rs:"));
}

const UNUSED: &[(&str, &str)] = &[
    (
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/member\"]\nresolver = \"3\"\n[workspace.dependencies]\norphan = { path = \"extras/orphan\" }\n",
    ),
    ("crates/member/Cargo.toml", MEMBER_CARGO),
    ("crates/member/src/lib.rs", MEMBER_LIB),
    (
        "extras/orphan/Cargo.toml",
        "[package]\nname = \"orphan\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    ),
    ("extras/orphan/src/lib.rs", GOOD),
];

#[tokio::test]
async fn review_unused_workspace_definition() {
    let workspace = Workspace::new(UNUSED);
    let gateway = CargoCompileGateway::with_files(UNUSED).await;
    let verdict = check(
        gateway.addr,
        &workspace.root(),
        &[("extras/orphan/src/lib.rs".into(), BAD.into())],
    )
    .await
    .unwrap();
    eprintln!("unused definition verdict: {verdict:?}");
    assert!(
        !verdict.passed,
        "unused definition cannot select unrelated members: {verdict:?}"
    );
    let runs = gateway.take_runs();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].request.subdir.as_deref(), Some("extras/orphan"));
    assert_eq!(runs[0].exit_code, Some(101));
    assert!(
        runs[0]
            .compiler_output
            .contains("believes it's in a workspace")
    );
}

const SUFFIX: &[(&str, &str)] = &[
    (
        "Cargo.toml",
        "[workspace]\nmembers = [\"member\"]\nexclude = [\"pkg\", \"nested/pkg\"]\nresolver = \"3\"\n",
    ),
    ("member/Cargo.toml", MEMBER_CARGO),
    ("member/src/lib.rs", MEMBER_LIB),
    (
        "pkg/Cargo.toml",
        "[package]\nname = \"one\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    ),
    ("pkg/src/lib.rs", GOOD),
    (
        "nested/pkg/Cargo.toml",
        "[package]\nname = \"two\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    ),
    ("nested/pkg/src/lib.rs", GOOD),
];

#[tokio::test]
async fn review_suffix_nested_diagnostics() {
    let workspace = Workspace::new(SUFFIX);
    let gateway = CargoCompileGateway::with_files(SUFFIX).await;
    for paths in [
        ["pkg/src/lib.rs", "nested/pkg/src/lib.rs"],
        ["nested/pkg/src/lib.rs", "pkg/src/lib.rs"],
    ] {
        let edits = paths.map(|p| (p.into(), BAD.into()));
        let verdict = check(gateway.addr, &workspace.root(), &edits)
            .await
            .unwrap();
        eprintln!("suffix verdict: {verdict:?}");
        let runs = gateway.take_runs();
        assert_eq!(runs.len(), 2);
        assert!(runs.iter().all(|r| r.exit_code == Some(101)));
        assert_eq!(verdict.errors.len(), 2, "distinct files: {verdict:?}");
        for path in paths {
            assert!(
                verdict
                    .errors
                    .iter()
                    .any(|e| e.starts_with(&format!("{path}:")))
            );
        }
    }
}

const PREFIX: &[(&str, &str)] = &[
    (
        "Cargo.toml",
        "[workspace]\nmembers = [\"member\"]\nexclude = [\"nested\"]\nresolver = \"3\"\n",
    ),
    ("member/Cargo.toml", MEMBER_CARGO),
    ("member/src/lib.rs", MEMBER_LIB),
    (
        "nested/Cargo.toml",
        "[package]\nname = \"prefix\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[lib]\npath = \"nested/lib.rs\"\n",
    ),
    ("nested/nested/lib.rs", GOOD),
];

#[tokio::test]
async fn review_cwd_prefix_is_relative() {
    let workspace = Workspace::new(PREFIX);
    let gateway = CargoCompileGateway::with_files(PREFIX).await;
    let verdict = check(
        gateway.addr,
        &workspace.root(),
        &[("nested/nested/lib.rs".into(), BAD.into())],
    )
    .await
    .unwrap();
    eprintln!("prefix verdict: {verdict:?}");
    let runs = gateway.take_runs();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].request.subdir.as_deref(), Some("nested"));
    assert_eq!(runs[0].exit_code, Some(101));
    assert_eq!(verdict.errors.len(), 1);
    assert!(
        verdict.errors[0].starts_with("nested/nested/lib.rs:"),
        "{verdict:?}"
    );
}
