/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::gateway::{CargoCompileGateway, assert_workspace_clean};
use super::{EXCLUDED_CARGO, MEMBER_CARGO, MEMBER_LIB};
use prod_code_mcp::compile_check::check;
use prod_code_testkit::Workspace;

const CROSS_GROUP_FILES: &[(&str, &str)] = &[
    (
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/member\"]\nexclude = [\"crates/excluded\", \"crates/excluded_two\"]\nresolver = \"3\"\n",
    ),
    ("crates/member/Cargo.toml", MEMBER_CARGO),
    ("crates/member/src/lib.rs", MEMBER_LIB),
    (
        "crates/excluded/Cargo.toml",
        "[package]\nname = \"excluded\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[dependencies]\nmember = { path = \"../member\" }\n",
    ),
    (
        "crates/excluded/src/lib.rs",
        "pub fn excluded_value() -> u32 { member::member_value() }\n",
    ),
    (
        "crates/excluded_two/Cargo.toml",
        "[package]\nname = \"excluded_two\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    ),
    (
        "crates/excluded_two/src/lib.rs",
        "pub fn second_excluded() -> u32 { 30 }\n",
    ),
];

#[tokio::test]
async fn two_distinct_edited_packages_check_all_proposed_contents_and_aggregate() {
    let workspace = Workspace::new(CROSS_GROUP_FILES);
    let gateway = CargoCompileGateway::with_files(CROSS_GROUP_FILES).await;

    // 1. Cross-package valid proposal: `crates/member` adds `newly_added()` and `crates/excluded`
    //    calls `member::newly_added()`. Both affected groups (`None` and `Some("crates/excluded")`)
    //    must receive ALL proposed contents so `crates/excluded` sees the proposed `member` API.
    let coupled_valid = vec![
        (
            "crates/member/src/lib.rs".to_string(),
            "pub fn member_value() -> u32 { 10 }\npub fn newly_added() -> u32 { 77 }\n".to_string(),
        ),
        (
            "crates/excluded/src/lib.rs".to_string(),
            "pub fn excluded_value() -> u32 { member::newly_added() }\n".to_string(),
        ),
    ];
    let ok_verdict = check(gateway.addr, &workspace.root(), &coupled_valid)
        .await
        .unwrap();
    assert!(ok_verdict.passed, "{ok_verdict:?}");
    assert!(ok_verdict.errors.is_empty());
    assert_eq!(ok_verdict.duration_ms, 30);

    let ok_runs = gateway.take_runs();
    assert_eq!(ok_runs.len(), 2);
    assert_eq!(ok_runs[0].request.subdir, None);
    assert_eq!(
        ok_runs[1].request.subdir.as_deref(),
        Some("crates/excluded")
    );
    for run in &ok_runs {
        let paths: Vec<_> = run.request.hypotheses[0]
            .files
            .iter()
            .map(|f| f.relative_path.as_str())
            .collect();
        assert_eq!(
            paths,
            ["crates/member/src/lib.rs", "crates/excluded/src/lib.rs"],
            "every affected group must receive all proposed contents"
        );
    }

    // 2. Both edited packages fail (`crates/member` with E0308 and `crates/excluded_two` with E0425):
    //    verdicts and diagnostics are aggregated across both groups.
    let both_broken = vec![
        (
            "crates/member/src/lib.rs".to_string(),
            "pub fn member_value() -> u32 { \"wrong_member_type\" }\n".to_string(),
        ),
        (
            "crates/excluded_two/src/lib.rs".to_string(),
            "pub fn second_excluded() -> u32 { missing_in_excluded() }\n".to_string(),
        ),
    ];
    let both_verdict = check(gateway.addr, &workspace.root(), &both_broken)
        .await
        .unwrap();
    assert!(!both_verdict.passed);
    assert_eq!(both_verdict.duration_ms, 30);
    assert!(
        both_verdict
            .errors
            .iter()
            .any(|e| e.contains("crates/member/src/lib.rs") && e.contains("E0308")),
        "missing member diagnostic in {:?}",
        both_verdict.errors
    );
    assert!(
        both_verdict
            .errors
            .iter()
            .any(|e| e.contains("crates/excluded_two/src/lib.rs") && e.contains("E0425")),
        "missing excluded_two diagnostic in {:?}",
        both_verdict.errors
    );
    let both_runs = gateway.take_runs();
    assert_eq!(both_runs.len(), 2);
    assert_eq!(both_runs[0].request.subdir, None);
    assert_eq!(
        both_runs[1].request.subdir.as_deref(),
        Some("crates/excluded_two")
    );
    assert!(both_runs[0].compiler_output.contains("E0308"));
    assert!(both_runs[1].compiler_output.contains("E0425"));

    // 3. Two distinct excluded packages (`crates/excluded` and `crates/excluded_two`):
    //    when the first passes and the second fails, the aggregated verdict fails with the second's error.
    let two_excluded = vec![
        (
            "crates/excluded/src/lib.rs".to_string(),
            "pub fn excluded_value() -> u32 { member::member_value() + 1 }\n".to_string(),
        ),
        (
            "crates/excluded_two/src/lib.rs".to_string(),
            "pub fn second_excluded() -> u32 { \"bad_second\" }\n".to_string(),
        ),
    ];
    let two_excluded_verdict = check(gateway.addr, &workspace.root(), &two_excluded)
        .await
        .unwrap();
    assert!(!two_excluded_verdict.passed);
    assert!(
        two_excluded_verdict
            .errors
            .iter()
            .any(|e| e.contains("crates/excluded_two/src/lib.rs") && e.contains("E0308")),
        "{:?}",
        two_excluded_verdict.errors
    );
    let two_excluded_runs = gateway.take_runs();
    assert_eq!(two_excluded_runs.len(), 2);
    assert_eq!(
        two_excluded_runs[0].request.subdir.as_deref(),
        Some("crates/excluded")
    );
    assert_eq!(
        two_excluded_runs[1].request.subdir.as_deref(),
        Some("crates/excluded_two")
    );

    assert_workspace_clean(
        &workspace,
        CROSS_GROUP_FILES,
        &[
            "crates/member/target",
            "crates/excluded/target",
            "crates/excluded_two/target",
        ],
    );
}

const DEDUP_WORKSPACE_FILES: &[(&str, &str)] = &[
    (
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/member_a\", \"crates/member_b\"]\nexclude = [\"crates/excluded\"]\nresolver = \"3\"\n",
    ),
    (
        "crates/member_a/Cargo.toml",
        "[package]\nname = \"member_a\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    ),
    ("crates/member_a/src/lib.rs", "pub fn a() -> u32 { 1 }\n"),
    (
        "crates/member_b/Cargo.toml",
        "[package]\nname = \"member_b\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    ),
    ("crates/member_b/src/lib.rs", "pub fn b() -> u32 { 2 }\n"),
    ("crates/excluded/Cargo.toml", EXCLUDED_CARGO),
    (
        "crates/excluded/src/lib.rs",
        "pub mod helper;\npub fn excluded_value() -> u32 { helper::value() }\n",
    ),
    (
        "crates/excluded/src/helper.rs",
        "pub fn value() -> u32 { 3 }\n",
    ),
];

#[tokio::test]
async fn duplicate_equivalent_commands_are_avoided() {
    let workspace = Workspace::new(DEDUP_WORKSPACE_FILES);
    let gateway = CargoCompileGateway::with_files(DEDUP_WORKSPACE_FILES).await;

    // 1. Two distinct member crates in the same workspace resolve to a single `subdir: None` run.
    let two_members = vec![
        (
            "crates/member_a/src/lib.rs".to_string(),
            "pub fn a() -> u32 { 11 }\n".to_string(),
        ),
        (
            "crates/member_b/src/lib.rs".to_string(),
            "pub fn b() -> u32 { 22 }\n".to_string(),
        ),
    ];
    let members_verdict = check(gateway.addr, &workspace.root(), &two_members)
        .await
        .unwrap();
    assert!(members_verdict.passed);
    let member_runs = gateway.take_runs();
    assert_eq!(
        member_runs.len(),
        1,
        "two members of the same Cargo workspace must not trigger duplicate commands"
    );
    assert_eq!(member_runs[0].request.subdir, None);
    assert_eq!(member_runs[0].request.hypotheses[0].files.len(), 2);

    // 2. Multiple files in the same excluded crate resolve to a single `subdir: Some("crates/excluded")` run.
    let two_excluded_files = vec![
        (
            workspace
                .path("crates/excluded/src/lib.rs")
                .to_string_lossy()
                .into_owned(),
            "pub mod helper;\npub fn excluded_value() -> u32 { helper::value() + 1 }\n".to_string(),
        ),
        (
            "crates/excluded/src/helper.rs".to_string(),
            "pub fn value() -> u32 { 40 }\n".to_string(),
        ),
    ];
    let excluded_verdict = check(gateway.addr, &workspace.root(), &two_excluded_files)
        .await
        .unwrap();
    assert!(excluded_verdict.passed);
    let excluded_runs = gateway.take_runs();
    assert_eq!(
        excluded_runs.len(),
        1,
        "multiple files in one excluded package must not trigger duplicate commands"
    );
    assert_eq!(
        excluded_runs[0].request.subdir.as_deref(),
        Some("crates/excluded")
    );
    assert_eq!(excluded_runs[0].request.hypotheses[0].files.len(), 2);

    assert_workspace_clean(
        &workspace,
        DEDUP_WORKSPACE_FILES,
        &[
            "crates/member_a/target",
            "crates/member_b/target",
            "crates/excluded/target",
        ],
    );
}
