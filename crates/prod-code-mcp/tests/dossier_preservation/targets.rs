/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::gateway::{Gateway, Reply, workspace};
use prod_code_mcp::dossier::diagnose;
use std::{path::Path, time::Duration};

#[tokio::test]
async fn test_build_fixes_keep_package_filter_and_exclude_benchmarks() {
    let temp = workspace();
    let gateway = Gateway::start(temp.path(), vec![Reply::Cargo, Reply::Cargo]).await;
    let result = diagnose(
        gateway.addr,
        temp.path(),
        Some(&temp.path().join("selected")),
        Some("chosen"),
        60,
    )
    .await;
    let calls = gateway.finish().await;
    let report = result.unwrap();
    println!(
        "build_errors={:?} suggested_fixes={:?}",
        report.build_errors, report.suggested_fixes
    );
    assert!(
        !report.build_errors.is_empty(),
        "must demonstrate a real test compilation failure"
    );
    assert!(
        report
            .suggested_fixes
            .iter()
            .any(|s| s.contains("selected/src/lib.rs") && s.contains("borrowing")),
        "retain the compiler's fix for the actual failing test"
    );
    assert!(
        !report
            .suggested_fixes
            .iter()
            .any(|s| s.contains("benches/")),
        "benchmark fixes leaked: {:?}",
        report.suggested_fixes
    );
    assert_eq!(calls.len(), 2);
    assert_eq!(
        calls[0].command,
        ["cargo", "test", "-p", "selected", "chosen"]
    );
    assert_eq!(
        calls[1].command,
        [
            "cargo",
            "test",
            "-p",
            "selected",
            "chosen",
            "--no-run",
            "--message-format=json"
        ]
    );
    assert!(calls.iter().all(|req| !req.pull_changes));

    // Apply only the real failing test's compiler-proposed borrow. A real test run
    // must now pass while the untouched benchmark remains a compiler error. This
    // proves target selection independently of a scripted clean diagnostic.
    let source = include_str!("fixtures/test_build.txt")
        .replace("consume(test_value)", "consume(&test_value)");
    std::fs::write(temp.path().join("selected/src/lib.rs"), source).unwrap();
    let benchmark = std::fs::read(temp.path().join("selected/benches/broken.rs")).unwrap();
    let passing = fixture_command(temp.path(), &calls[0].command).await;
    assert!(passing.status.success());
    assert!(String::from_utf8_lossy(&passing.stdout).contains("test tests::chosen ... ok"));
    let all_targets: Vec<_> = ["cargo", "check", "-p", "selected", "--all-targets"]
        .map(String::from)
        .into();
    let still_broken = fixture_command(temp.path(), &all_targets).await;
    assert_eq!(still_broken.status.code(), Some(101));
    assert!(String::from_utf8_lossy(&still_broken.stderr).contains("benches/broken.rs"));
    assert_eq!(
        std::fs::read(temp.path().join("selected/benches/broken.rs")).unwrap(),
        benchmark
    );
}

async fn fixture_command(root: &Path, command: &[String]) -> std::process::Output {
    let mut child = tokio::process::Command::new(&command[0]);
    child
        .args(&command[1..])
        .current_dir(root)
        .env("CARGO_BUILD_JOBS", "2")
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(30), child.output())
        .await
        .unwrap()
        .unwrap();
    println!(
        "fixture command={command:?} exit={:?}\n{}\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout)
            .replace(&root.to_string_lossy().to_string(), "<fixture>"),
        String::from_utf8_lossy(&output.stderr)
            .replace(&root.to_string_lossy().to_string(), "<fixture>")
    );
    output
}

#[test]
fn ordinary_check_and_test_plans_are_unchanged() {
    use prod_code_mcp::verify::{VerifyKind, plan_command};
    assert_eq!(
        plan_command("rust", VerifyKind::Check, None).unwrap(),
        [
            "cargo",
            "check",
            "--workspace",
            "--all-targets",
            "--message-format=json"
        ]
    );
    assert_eq!(
        plan_command("rust", VerifyKind::Test, Some("chosen")).unwrap(),
        ["cargo", "test", "--workspace", "chosen"]
    );
}
