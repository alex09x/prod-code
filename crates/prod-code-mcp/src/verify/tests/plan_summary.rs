/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::verify::plan::*;
use crate::verify::types::*;

#[test]
fn plans_and_summary() {
    assert_eq!(
        plan_command("rust", VerifyKind::Test, Some("sync::"))
            .unwrap()
            .last()
            .unwrap(),
        "sync::"
    );
    assert_eq!(
        plan_command("go", VerifyKind::Test, Some("TestA")).unwrap()[3..],
        ["./...", "-run", "TestA"]
    );
    assert_eq!(
        plan_command("ruby", VerifyKind::Check, None).unwrap(),
        vec!["bundle", "exec", "rake", "test"]
    );
    assert_eq!(
        plan_command("dart", VerifyKind::Check, None).unwrap(),
        vec!["dart", "analyze"]
    );
    assert_eq!(
        plan_command("dart", VerifyKind::Test, Some("my_test")).unwrap(),
        vec!["dart", "test", "--name", "my_test"]
    );
    assert_eq!(
        plan_command("zig", VerifyKind::Check, None).unwrap(),
        vec!["zig", "build"]
    );
    assert!(plan_command("zig", VerifyKind::Test, Some("foo")).is_err());
    assert_eq!(
        plan_command("elixir", VerifyKind::Check, None).unwrap(),
        vec!["mix", "compile"]
    );
    assert_eq!(
        plan_command("elixir", VerifyKind::Test, Some("tag")).unwrap(),
        vec!["mix", "test", "--only", "tag"]
    );
    assert_eq!(
        plan_command("scala", VerifyKind::Check, None).unwrap(),
        vec!["sbt", "compile"]
    );
    assert_eq!(
        plan_command("scala", VerifyKind::Test, Some("Spec")).unwrap(),
        vec!["sbt", "test", "testOnly *Spec"]
    );
    assert_eq!(
        plan_command("lua", VerifyKind::Check, None).unwrap(),
        vec!["luacheck", "."]
    );
    assert_eq!(
        plan_command("lua", VerifyKind::Test, Some("suite")).unwrap(),
        vec!["busted", "--filter", "suite"]
    );
    assert!(plan_command("unknown_language", VerifyKind::Check, None).is_err());
    let report = VerifyReport {
        kind: VerifyKind::Test,
        language: "rust".into(),
        command: vec!["cargo".into(), "test".into()],
        exit_code: Some(101),
        timed_out: false,
        duration_ms: 1500,
        diagnostics: vec![],
        tests_passed: 2,
        tests_failed: 1,
        failures: vec![TestFailure {
            name: "a::bad".into(),
            output: "boom\n".into(),
        }],
        tail: String::new(),
        fixes: vec![],
        benches: vec![],
        usage: None,
        platform: None,
        executor: None,
    };
    assert_eq!(
        report.summary(),
        "rust test: FAILED (exit 101) in 1.5s; 2 passed, 1 failed"
    );
    assert!(report.render(10).contains("--- FAILED a::bad ---\nboom"));
}
