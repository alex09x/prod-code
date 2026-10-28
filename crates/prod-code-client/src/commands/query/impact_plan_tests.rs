/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::impact_run_plan;
use prod_code_mcp::impact::{Gap, ImpactReport, ImpactTestPlan, Symbol};

fn report(plan: ImpactTestPlan) -> ImpactReport {
    let test = Symbol {
        name: "test".into(),
        file: "zig/test.zig".into(),
        line: 1,
        col: 1,
    };
    ImpactReport {
        language: "rust".into(),
        base: "HEAD".into(),
        changed_files: vec!["src/lib.rs".into()],
        changed: vec![test.clone()],
        callers: vec![],
        tests: vec![test],
        test_command: plan.selected.clone(),
        test_plan: Some(plan),
        unattributed_files: vec![],
        index: None,
        reaches: vec![],
        incomplete: vec![],
        signature_warnings: vec![],
    }
}

#[test]
fn impact_run_and_ci_refuse_unplanned_project_instead_of_root_only_suite() {
    let root = tempfile::tempdir().unwrap();
    let mut report = report(ImpactTestPlan {
        unsupported_selected: vec!["zig (zig)".into()],
        unsupported_whole: vec!["zig (zig)".into()],
        ..Default::default()
    });
    let error = impact_run_plan(root.path(), &report)
        .unwrap_err()
        .to_string();
    assert!(error.contains("zig (zig)"), "{error}");
    report.incomplete.push(Gap::Timeout {
        elapsed_ms: 2,
        budget_ms: 1,
    });
    let error = impact_run_plan(root.path(), &report)
        .unwrap_err()
        .to_string();
    assert!(error.contains("zig (zig)"), "{error}");
}

#[test]
fn impact_run_and_ci_keep_composed_project_commands() {
    let root = tempfile::tempdir().unwrap();
    let command = vec![
        "sh".into(),
        "-c".into(),
        "cargo test && (cd java && ./gradlew test)".into(),
    ];
    let mut report = report(ImpactTestPlan {
        selected: Some(command.clone()),
        selection_fallbacks: vec!["java (java)".into()],
        whole: Some(command.clone()),
        ..Default::default()
    });
    let (selected, why) = impact_run_plan(root.path(), &report).unwrap();
    assert_eq!(selected, Some(command.clone()));
    assert!(why.contains("java (java)"));
    report.incomplete.push(Gap::Timeout {
        elapsed_ms: 2,
        budget_ms: 1,
    });
    let (whole, _) = impact_run_plan(root.path(), &report).unwrap();
    assert_eq!(whole, Some(command));
}
