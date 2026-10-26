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
use super::{MEMBER_CARGO, MEMBER_LIB};
use prod_code_mcp::compile_check::check;
use prod_code_testkit::Workspace;

const GOOD: &str = "pub fn value() -> u32 { 1 }\n";
const BAD: &str = "pub fn value() -> u32 { \"wrong\" }\n";
const SHARED: &[(&str, &str)] = &[
    (
        "Cargo.toml",
        "[workspace]\nmembers = [\"member\"]\nexclude = [\"excluded\"]\nresolver = \"3\"\n",
    ),
    (
        "member/Cargo.toml",
        "[package]\nname = \"member\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[lib]\npath = \"../shared/entry.rs\"\n",
    ),
    (
        "excluded/Cargo.toml",
        "[package]\nname = \"excluded\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[lib]\npath = \"../shared/entry.rs\"\n[[test]]\nname = \"alias\"\npath = \"../shared/entry.rs\"\n",
    ),
    (
        "shared/entry.rs",
        "pub mod helper;\n#[path = \"other.rs\"]\npub mod different;\n",
    ),
    ("shared/helper.rs", GOOD),
    ("shared/other.rs", GOOD),
    ("shared/unlinked.rs", GOOD),
];

#[tokio::test]
async fn shared_targets_modules_aliases_and_unknown_ownership() {
    let workspace = Workspace::new(SHARED);
    let gateway = CargoCompileGateway::with_files(SHARED).await;
    for file in ["shared/entry.rs", "shared/helper.rs", "shared/other.rs"] {
        let verdict = check(
            gateway.addr,
            &workspace.root(),
            &[(file.into(), BAD.into())],
        )
        .await
        .unwrap();
        eprintln!("shared {file}: {verdict:?}");
        assert!(!verdict.passed);
        let runs = gateway.take_runs();
        assert_eq!(runs.len(), 2, "all owning groups for {file}");
        assert!(runs.iter().any(|r| r.request.subdir.is_none()));
        assert!(
            runs.iter()
                .any(|r| r.request.subdir.as_deref() == Some("excluded"))
        );
        assert!(runs.iter().all(|r| r.exit_code == Some(101)));
        assert_eq!(
            verdict.errors.len(),
            1,
            "same resolved source must deduplicate: {verdict:?}"
        );
        assert!(verdict.errors[0].starts_with(&format!("{file}:")));
    }
    // Valid external targets and aliases remain accepted.
    let valid = check(
        gateway.addr,
        &workspace.root(),
        &[("shared/helper.rs".into(), GOOD.into())],
    )
    .await
    .unwrap();
    assert!(valid.passed, "{valid:?}");
    assert_eq!(gateway.take_runs().len(), 2);
    let unknown = check(
        gateway.addr,
        &workspace.root(),
        &[("shared/unlinked.rs".into(), BAD.into())],
    )
    .await
    .unwrap();
    assert!(!unknown.passed);
    assert!(
        unknown
            .errors
            .iter()
            .any(|e| e.contains("incomplete compiler verification"))
    );
    assert!(
        gateway.take_runs().is_empty(),
        "unrelated root command is not ownership proof"
    );
}

const INHERITED: &[(&str, &str)] = &[
    (
        "Cargo.toml",
        "[workspace]\nmembers = [\"member\"]\nresolver = \"3\"\n[workspace.dependencies]\nalias = { package = \"inherited\", path = \"deps/inherited\" }\n",
    ),
    (
        "member/Cargo.toml",
        "[package]\nname = \"member\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[target.'cfg(unix)'.dependencies]\nalias.workspace = true\n",
    ),
    (
        "member/src/lib.rs",
        "pub fn member_value() -> u32 { alias::value() }\n",
    ),
    (
        "deps/inherited/Cargo.toml",
        "[package]\nname = \"inherited\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[dependencies]\nleaf = { path = \"../leaf\" }\n",
    ),
    (
        "deps/inherited/src/lib.rs",
        "pub fn value() -> u32 { leaf::value() }\n",
    ),
    (
        "deps/leaf/Cargo.toml",
        "[package]\nname = \"leaf\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    ),
    ("deps/leaf/src/lib.rs", GOOD),
];

#[tokio::test]
async fn consumed_inherited_and_transitive_path_edges_remain_workspace_members() {
    let workspace = Workspace::new(INHERITED);
    let gateway = CargoCompileGateway::with_files(INHERITED).await;
    for file in ["deps/inherited/src/lib.rs", "deps/leaf/src/lib.rs"] {
        let verdict = check(
            gateway.addr,
            &workspace.root(),
            &[(file.into(), BAD.into())],
        )
        .await
        .unwrap();
        assert!(!verdict.passed, "{verdict:?}");
        let runs = gateway.take_runs();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].request.subdir, None);
        assert_eq!(runs[0].exit_code, Some(101));
        assert!(verdict.errors.iter().any(|e| e.starts_with(file)));
    }
}

const TARGET_KINDS: &[(&str, &str)] = &[
    (
        "Cargo.toml",
        "[workspace]\nmembers = [\"member\"]\nexclude = [\"excluded\"]\nresolver = \"3\"\n",
    ),
    ("member/Cargo.toml", MEMBER_CARGO),
    ("member/src/lib.rs", MEMBER_LIB),
    (
        "excluded/Cargo.toml",
        "[package]\nname = \"kinds\"\nversion = \"0.1.0\"\nedition = \"2024\"\nbuild = \"../shared/build.rs\"\n[[bin]]\nname = \"bin_alias\"\npath = \"../shared/bin.rs\"\n[[test]]\nname = \"test_alias\"\npath = \"../shared/test.rs\"\n[[example]]\nname = \"example_alias\"\npath = \"../shared/example.rs\"\n[[bench]]\nname = \"bench_alias\"\npath = \"../shared/bench.rs\"\n",
    ),
    ("shared/build.rs", "fn main() {}\n"),
    ("shared/bin.rs", "fn main() {}\n"),
    ("shared/test.rs", GOOD),
    ("shared/example.rs", "fn main() {}\n"),
    ("shared/bench.rs", GOOD),
];

const ABSOLUTE_DIAGNOSTICS: &[(&str, &str)] = &[
    (
        "Cargo.toml",
        "[workspace]\nmembers = [\"member\"]\nexclude = [\"excluded\"]\nresolver = \"3\"\n",
    ),
    ("member/Cargo.toml", MEMBER_CARGO),
    (
        "member/src/lib.rs",
        "include!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/../shared.rs\"));\n",
    ),
    (
        "excluded/Cargo.toml",
        "[package]\nname = \"excluded\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[lib]\npath = \"../shared.rs\"\n",
    ),
    ("shared.rs", GOOD),
];

#[tokio::test]
async fn absolute_and_relative_diagnostics_share_explicit_server_identity() {
    let workspace = Workspace::new(ABSOLUTE_DIAGNOSTICS);
    let gateway = CargoCompileGateway::with_files(ABSOLUTE_DIAGNOSTICS).await;
    let edits = [
        ("member/src/lib.rs".into(), ABSOLUTE_DIAGNOSTICS[2].1.into()),
        ("shared.rs".into(), BAD.into()),
    ];
    let verdict = check(gateway.addr, &workspace.root(), &edits)
        .await
        .unwrap();
    eprintln!("absolute identity verdict: {verdict:?}");
    let runs = gateway.take_runs();
    assert_eq!(runs.len(), 2);
    assert!(runs.iter().all(|r| r.exit_code == Some(101)));
    assert!(
        runs[0].compiler_output.contains("/../shared.rs:1:"),
        "real absolute compiler path required"
    );
    assert_eq!(
        verdict.errors.len(),
        1,
        "same file from two explicit origins: {verdict:?}"
    );
    assert!(verdict.errors[0].starts_with("shared.rs:"));
}

#[tokio::test]
async fn explicit_external_target_kinds_are_not_rejected() {
    let workspace = Workspace::new(TARGET_KINDS);
    let gateway = CargoCompileGateway::with_files(TARGET_KINDS).await;
    for file in [
        "shared/build.rs",
        "shared/bin.rs",
        "shared/test.rs",
        "shared/example.rs",
        "shared/bench.rs",
    ] {
        let verdict = check(
            gateway.addr,
            &workspace.root(),
            &[(file.into(), format!("{BAD}fn main() {{}}\n"))],
        )
        .await
        .unwrap();
        assert!(!verdict.passed, "{file}: {verdict:?}");
        let runs = gateway.take_runs();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].request.subdir.as_deref(), Some("excluded"));
        assert_eq!(runs[0].exit_code, Some(101));
        assert!(
            verdict
                .errors
                .iter()
                .any(|e| e.starts_with(file) && e.contains("E0308")),
            "{verdict:?}"
        );
    }
}
