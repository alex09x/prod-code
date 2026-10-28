/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::impact::{CiRun, Gap, ImpactReport, Symbol, test_command_for_tests};

#[test]
#[cfg(unix)]
fn selectorless_java_group_keeps_its_project_suite_alongside_rust() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("Cargo.toml"),
        "[package]\nname='demo'\nversion='0.1.0'\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.path().join("src")).unwrap();
    std::fs::write(
        root.path().join("src/lib.rs"),
        "#[test] fn rust_test() {}\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.path().join("java/src/test")).unwrap();
    std::fs::write(
        root.path().join("java/build.gradle"),
        "plugins { id 'java' }\n",
    )
    .unwrap();
    let wrapper = root.path().join("java/gradlew");
    std::fs::write(&wrapper, "#!/bin/sh\nprintf 'java-suite-ran\\n'\n").unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(
        root.path().join("java/src/test/Example.java"),
        "class Example {}\n",
    )
    .unwrap();
    let tests = vec![
        Symbol {
            name: "rust_test".into(),
            file: "src/lib.rs".into(),
            line: 1,
            col: 1,
        },
        Symbol {
            name: "java_test".into(),
            file: "java/src/test/Example.java".into(),
            line: 1,
            col: 1,
        },
    ];
    let tools = crate::verify::detect_tools(root.path());
    let plan = test_command_for_tests(root.path(), "rust", &tools, &tests)
        .expect("both projects must remain in the test plan");
    let script = plan.join(" ");
    assert!(
        script.contains("cargo") && script.contains("rust_test"),
        "{script}"
    );
    assert!(
        script.contains("java") && script.contains("gradle") && script.contains("test"),
        "{script}"
    );
    let output = std::process::Command::new(&plan[0])
        .args(&plan[1..])
        .current_dir(root.path())
        .env("CARGO_TARGET_DIR", root.path().join("target"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("rust_test ... ok") && stdout.contains("java-suite-ran"),
        "{stdout}"
    );
    let plan = crate::impact::test_plan::build_test_plan(
        root.path(),
        "rust",
        &tools,
        &tests,
        &[
            "src/lib.rs".into(),
            "Cargo.toml".into(),
            "java/build.gradle".into(),
        ],
    );
    assert_eq!(plan.selection_fallbacks, ["java (java)"]);
    assert!(
        plan.unsupported_whole.is_empty(),
        "{:?}",
        plan.unsupported_whole
    );
    let mut report = impact_report(tests, plan);
    let decision = report.ci_decision();
    assert!(matches!(decision.run, CiRun::Selected(_)));
    assert!(
        decision.why.contains("whole suites for java (java)"),
        "{}",
        decision.why
    );
    report.incomplete.push(Gap::Timeout {
        elapsed_ms: 2,
        budget_ms: 1,
    });
    let full = report.ci_decision();
    let CiRun::PlannedSuite(command) = full.run else {
        panic!("{full:?}")
    };
    let command = command.join(" ");
    assert!(
        command.contains("cargo") && command.contains("gradle"),
        "{command}"
    );
}

fn impact_report(tests: Vec<Symbol>, plan: crate::impact::ImpactTestPlan) -> ImpactReport {
    ImpactReport {
        language: "rust".into(),
        base: "HEAD".into(),
        changed_files: vec!["src/lib.rs".into()],
        changed: vec![Symbol {
            name: "changed".into(),
            file: "src/lib.rs".into(),
            line: 1,
            col: 1,
        }],
        callers: vec![],
        test_command: plan.selected.clone(),
        test_plan: Some(plan),
        tests,
        unattributed_files: vec![],
        index: None,
        reaches: vec![],
        incomplete: vec![],
        signature_warnings: vec![],
    }
}

#[test]
fn unsupported_project_never_becomes_a_root_only_success() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("Cargo.toml"),
        "[package]\nname='demo'\nversion='0.1.0'\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.path().join("src")).unwrap();
    std::fs::write(root.path().join("src/lib.rs"), "fn rust_test() {}\n").unwrap();
    std::fs::create_dir_all(root.path().join("zig")).unwrap();
    std::fs::write(root.path().join("zig/build.zig"), "// no test step\n").unwrap();
    std::fs::write(root.path().join("zig/test.zig"), "test {}\n").unwrap();
    let tests = vec![
        Symbol {
            name: "rust_test".into(),
            file: "src/lib.rs".into(),
            line: 1,
            col: 1,
        },
        Symbol {
            name: "zig_test".into(),
            file: "zig/test.zig".into(),
            line: 1,
            col: 1,
        },
    ];
    let tools = crate::verify::detect_tools(root.path());
    let plan = crate::impact::test_plan::build_test_plan(
        root.path(),
        "rust",
        &tools,
        &tests,
        &["src/lib.rs".into()],
    );
    assert_eq!(plan.unsupported_selected, ["zig (zig)"]);
    assert!(plan.selected.is_none());
    let mut report = impact_report(tests, plan);
    assert_eq!(report.ci_decision().run, CiRun::Unsupported);
    assert!(report.render().contains("cannot plan complete coverage"));
    report.incomplete.push(Gap::Timeout {
        elapsed_ms: 2,
        budget_ms: 1,
    });
    let decision = report.ci_decision();
    assert_eq!(decision.run, CiRun::Unsupported);
    assert!(decision.why.contains("zig (zig)"), "{}", decision.why);
}

#[test]
fn supported_go_and_typescript_groups_keep_their_selectors() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("Cargo.toml"),
        "[package]\nname='demo'\nversion='0.1.0'\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.path().join("go")).unwrap();
    std::fs::write(
        root.path().join("go/go.mod"),
        "module example.test/demo\n\ngo 1.22\n",
    )
    .unwrap();
    std::fs::write(root.path().join("go/service_test.go"), "package demo\n").unwrap();
    std::fs::create_dir_all(root.path().join("web")).unwrap();
    std::fs::write(
        root.path().join("web/package.json"),
        r#"{"scripts":{"test":"vitest run"},"devDependencies":{"vitest":"1.0.0"}}"#,
    )
    .unwrap();
    std::fs::write(
        root.path().join("web/service.test.ts"),
        "test('works', () => {});\n",
    )
    .unwrap();
    let tests = vec![
        Symbol {
            name: "TestService".into(),
            file: "go/service_test.go".into(),
            line: 1,
            col: 1,
        },
        Symbol {
            name: "works".into(),
            file: "web/service.test.ts".into(),
            line: 1,
            col: 1,
        },
    ];
    let tools = crate::verify::detect_tools(root.path());
    let plan = crate::impact::test_plan::build_test_plan(root.path(), "rust", &tools, &tests, &[]);
    assert!(
        plan.unsupported_selected.is_empty(),
        "{:?}",
        plan.unsupported_selected
    );
    let command = plan.selected.unwrap().join(" ");
    assert!(
        command.contains("go test") && command.contains("TestService"),
        "{command}"
    );
    assert!(
        command.contains("web") && command.contains("vitest") && command.contains("works"),
        "{command}"
    );
}

#[test]
fn non_source_changes_keep_enclosing_projects_without_bogus_engines() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("Cargo.toml"),
        "[package]\nname='demo'\nversion='0.1.0'\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.path().join("web")).unwrap();
    std::fs::write(
        root.path().join("web/package.json"),
        r#"{"scripts":{"test":"vitest run"},"devDependencies":{"vitest":"1.0.0"}}"#,
    )
    .unwrap();
    std::fs::write(root.path().join("web/tsconfig.json"), "{}\n").unwrap();
    std::fs::create_dir_all(root.path().join("python")).unwrap();
    std::fs::write(
        root.path().join("python/pyproject.toml"),
        "[project]\nname='demo'\nversion='0.1.0'\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.path().join("scripts")).unwrap();
    std::fs::write(root.path().join("scripts/check.sh"), "#!/bin/sh\n").unwrap();
    std::fs::write(root.path().join("Dockerfile"), "FROM scratch\n").unwrap();
    let files = [
        "Cargo.toml",
        "web/package.json",
        "web/tsconfig.json",
        "python/pyproject.toml",
        "scripts/check.sh",
        "Dockerfile",
    ]
    .map(str::to_string);
    let tools = crate::verify::detect_tools(root.path());
    let plan = crate::impact::test_plan::build_test_plan(root.path(), "rust", &tools, &[], &files);
    assert!(
        plan.unsupported_whole.is_empty(),
        "{:?}",
        plan.unsupported_whole
    );
    let mut report = impact_report(vec![], plan);
    report.unattributed_files = vec!["web/package.json".into()];
    let decision = report.ci_decision();
    let CiRun::PlannedSuite(command) = decision.run else {
        panic!("{decision:?}")
    };
    let script = command.join(" ");
    assert!(
        script.contains("cargo")
            && script.contains("(cd web &&")
            && script.contains("vitest")
            && script.contains("(cd python &&"),
        "{script}"
    );
}

#[test]
fn empty_scope_fallback_still_plans_the_root_suite() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("Cargo.toml"),
        "[package]\nname='demo'\nversion='0.1.0'\n",
    )
    .unwrap();
    let tools = crate::verify::detect_tools(root.path());
    let plan = crate::impact::test_plan::build_test_plan(root.path(), "rust", &tools, &[], &[]);
    assert!(plan.whole_is_root_only);
    assert!(plan.whole.unwrap().contains(&"cargo".to_string()));
}

#[test]
fn external_paths_do_not_become_executable_project_scopes() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("Cargo.toml"),
        "[package]\nname='demo'\nversion='0.1.0'\n",
    )
    .unwrap();
    let tools = crate::verify::detect_tools(root.path());
    let plan = crate::impact::test_plan::build_test_plan(
        root.path(),
        "rust",
        &tools,
        &[],
        &["../outside/Cargo.toml".into()],
    );
    assert!(plan.whole.is_none());
    assert_eq!(plan.unsupported_whole, ["../outside/Cargo.toml (unknown)"]);
}
