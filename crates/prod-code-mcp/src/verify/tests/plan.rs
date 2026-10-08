/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::verify::detect::detect_tools;
use crate::verify::parse::*;
use crate::verify::plan::*;
use crate::verify::types::*;

#[test]
fn narrow_scope_picks_cargo_member_go_dir_and_pytest_path() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/*\"]\n",
    )
    .unwrap();
    let member = root.join("crates/gw/src");
    std::fs::create_dir_all(&member).unwrap();
    std::fs::write(
        root.join("crates/gw/Cargo.toml"),
        "[package]\nname = \"gw\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    std::fs::write(member.join("lib.rs"), "").unwrap();

    let mut cmd = strs(&["cargo", "test", "--workspace"]);
    narrow_scope(&mut cmd, "rust", root, &member.join("lib.rs"));
    assert_eq!(cmd, strs(&["cargo", "test", "-p", "gw"]));

    let mut cmd = strs(&["cargo", "test", "--workspace"]);
    narrow_scope(&mut cmd, "rust", root, root);
    assert_eq!(cmd, strs(&["cargo", "test", "--workspace"]));

    // A crate name instead of a path.
    let mut cmd = strs(&["cargo", "test", "--workspace"]);
    narrow_scope(&mut cmd, "rust", root, &root.join("gw"));
    assert_eq!(cmd, strs(&["cargo", "test", "-p", "gw"]));

    let mut cmd = strs(&["go", "test", "-json", "./..."]);
    narrow_scope(&mut cmd, "go", root, &root.join("crates/gw"));
    assert_eq!(cmd, strs(&["go", "test", "-json", "./crates/gw/..."]));

    // Go's lint script takes its packages as an argument, which narrows the same way.
    let mut cmd = plan_command("go", VerifyKind::Lint, None).unwrap();
    narrow_scope(&mut cmd, "go", root, &root.join("crates/gw"));
    assert_eq!(cmd.last().unwrap(), "./crates/gw/...");

    let mut cmd = strs(&["python3", "-m", "pytest", "-q"]);
    narrow_scope(&mut cmd, "python", root, &member.join("lib.rs"));
    assert_eq!(cmd.last().unwrap(), "crates/gw/src/lib.rs");
}

#[test]
fn detects_js_and_python_tooling() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::write(
        root.join("package.json"),
        r#"{"devDependencies":{"vitest":"1","eslint":"9"}}"#,
    )
    .unwrap();
    std::fs::write(root.join("bun.lock"), "").unwrap();
    std::fs::write(root.join("uv.lock"), "").unwrap();
    std::fs::write(root.join("meson.build"), "").unwrap();
    let tools = detect_tools(root);
    assert_eq!(tools.package_manager, PackageManager::Bun);
    assert_eq!(tools.js_tests, JsTestRunner::Vitest);
    assert_eq!(tools.js_linter, Some("eslint"));
    assert_eq!(tools.python, PythonRuntime::Uv);
    assert_eq!(tools.cpp, CppBuild::Meson);
    let cmd = plan_command_with(&tools, "typescript", VerifyKind::Test, Some("adds")).unwrap();
    assert_eq!(cmd[..3], ["bunx", "vitest", "run"]);
    assert!(cmd.ends_with(&["-t".to_string(), "adds".to_string()]));
    let cmd = plan_command_with(&tools, "python", VerifyKind::Check, None).unwrap();
    assert_eq!(cmd[..2], ["uv", "run"]);
    let cmd = plan_command_with(&tools, "python", VerifyKind::Test, None).unwrap();
    assert_eq!(cmd[..5], ["uv", "run", "python", "-m", "pytest"]);

    std::fs::remove_file(root.join("bun.lock")).unwrap();
    std::fs::remove_file(root.join("uv.lock")).unwrap();
    std::fs::write(root.join("pnpm-lock.yaml"), "").unwrap();
    std::fs::create_dir_all(root.join(".venv/bin")).unwrap();
    std::fs::write(root.join(".venv/bin/python"), "").unwrap();
    std::fs::create_dir_all(root.join("tests")).unwrap();
    std::fs::write(root.join("tests/test_a.py"), "import unittest\n").unwrap();
    let tools = detect_tools(root);
    assert_eq!(tools.package_manager, PackageManager::Pnpm);
    assert_eq!(tools.python, PythonRuntime::Venv(".venv/bin/python".into()));
    assert_eq!(tools.python_tests, PythonTestRunner::Unittest);
    let cmd = plan_command_with(&tools, "python", VerifyKind::Test, None).unwrap();
    assert_eq!(cmd[..3], [".venv/bin/python", "-m", "unittest"]);
    let cmd = plan_command_with(&tools, "python", VerifyKind::Check, None).unwrap();
    assert!(cmd.contains(&"--pythonpath".to_string()));

    let defaults = ProjectTools::default();
    assert_eq!(
        plan_command_with(&defaults, "typescript", VerifyKind::Check, None).unwrap()[..3],
        ["npx", "--no-install", "tsc"]
    );
    assert!(plan_command_with(&defaults, "typescript", VerifyKind::Lint, None).is_err());
}

#[test]
fn parses_js_and_python_test_runners() {
    let jest = "PASS src/a.test.ts\nFAIL src/b.test.ts\n  ● math › adds\n\n    expect(received).toBe(expected)\n\nTests:       1 failed, 1 passed, 2 total\n";
    let (p, f, fails) = parse_jest_text(jest);
    assert_eq!((p, f), (1, 1));
    assert_eq!(fails[0].name, "math › adds");
    assert!(fails[0].output.contains("expect(received)"));

    let vitest = " ✓ src/a.test.ts (1)\n ❯ src/b.test.ts (1)\n   × adds\n\n FAIL  src/b.test.ts > adds\nAssertionError: expected 2 to be 3\n\n Test Files  1 failed | 1 passed (2)\n      Tests  1 failed | 1 passed (2)\n";
    let (p, f, fails) = parse_vitest_text(vitest);
    assert_eq!((p, f), (1, 1));
    assert_eq!(fails[0].name, "src/b.test.ts > adds");
    assert!(fails[0].output.contains("AssertionError"));

    let bun = "bun test v1.4.2\n\nsrc/a.test.ts:\n(pass) adds\n(fail) subtracts [0.10ms]\nerror: expect(received).toBe(expected)\n\n 1 pass\n 1 fail\nRan 2 tests across 1 file.\n";
    let (p, f, fails) = parse_bun_test_text(bun);
    assert_eq!((p, f), (1, 1));
    assert_eq!(fails[0].name, "subtracts");
    assert!(fails[0].output.contains("expect(received)"));

    let unittest = "test_adds (tests.test_a.T.test_adds) ... ok\ntest_subs (tests.test_a.T.test_subs) ... FAIL\n\n======================================================================\nFAIL: test_subs (tests.test_a.T.test_subs)\n----------------------------------------------------------------------\nTraceback (most recent call last):\nAssertionError: 2 != 3\n\n----------------------------------------------------------------------\nRan 2 tests in 0.001s\n\nFAILED (failures=1)\n";
    let (p, f, fails) = parse_unittest_text(unittest);
    assert_eq!((p, f), (1, 1));
    assert_eq!(fails[0].name, "test_subs (tests.test_a.T.test_subs)");
    assert!(fails[0].output.contains("AssertionError: 2 != 3"));

    let meson = "1/2 adds        OK              0.01s\n2/2 subs        FAIL            0.02s   exit status 1\n\nOk:                 1\nFail:               1\n";
    let (p, f, fails) = parse_meson_test_text(meson);
    assert_eq!((p, f), (1, 1));
    assert_eq!(fails[0].name, "subs");
}

#[test]
fn plans_xcodebuild_commands() {
    let cmd = plan_xcode_command(VerifyKind::Test, Some("Tako")).unwrap();
    assert_eq!(&cmd[..2], &["sh".to_string(), "-c".to_string()]);
    assert!(cmd[2].contains("scheme='Tako'"));
    assert!(cmd[2].contains("xcodebuild test -scheme"));
    let cmd = plan_xcode_command(VerifyKind::Check, None).unwrap();
    assert!(cmd[2].contains("xcodebuild -list -json"));
    assert!(cmd[2].contains("xcodebuild build -scheme"));
    assert!(plan_xcode_command(VerifyKind::Lint, None).is_err());
    let temp = tempfile::tempdir().unwrap();
    assert!(!has_xcode_project(temp.path()));
    std::fs::create_dir_all(temp.path().join("App.xcodeproj")).unwrap();
    assert!(has_xcode_project(temp.path()));
    std::fs::write(temp.path().join("Package.swift"), "").unwrap();
    assert!(!has_xcode_project(temp.path()));
}

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
    };
    assert_eq!(
        report.summary(),
        "rust test: FAILED (exit 101) in 1.5s; 2 passed, 1 failed"
    );
    assert!(report.render(10).contains("--- FAILED a::bad ---\nboom"));
}

#[test]
fn zig_build_projects_use_a_test_plan() {
    let root = tempfile::tempdir().unwrap();
    let build_file = root.path().join("build.zig");
    std::fs::write(
        &build_file,
        "const test_step = b.step(\"test\", \"Run tests\");",
    )
    .unwrap();
    let tools = detect_tools(root.path());

    assert!(tools.zig_test_step);
    assert_eq!(
        plan_command_with(&tools, "zig", VerifyKind::Test, None).unwrap(),
        vec!["zig", "build", "test"]
    );
    assert!(plan_command_with(&tools, "zig", VerifyKind::Test, Some("foo")).is_err());

    std::fs::write(
        &build_file,
        "const install_step = b.step(\"install\", \"Install\");",
    )
    .unwrap();
    let tools = detect_tools(root.path());
    assert!(!tools.zig_test_step);
    let error = plan_command_with(&tools, "zig", VerifyKind::Test, None).unwrap_err();
    assert!(error.to_string().contains("test step"));

    std::fs::write(
        &build_file,
        "// const test_step = b.step(\"test\", \"comment only\");",
    )
    .unwrap();
    assert!(!detect_tools(root.path()).zig_test_step);

    std::fs::write(
        &build_file,
        "const s =\n    \\\\.step(\"test\", \"string only\")\n;",
    )
    .unwrap();
    assert!(!detect_tools(root.path()).zig_test_step);

    std::fs::write(
        &build_file,
        "const s =\n    c\\\\.step(\"test\", \"c string only\")\n;",
    )
    .unwrap();
    assert!(!detect_tools(root.path()).zig_test_step);

    assert!(plan_command_with(&ProjectTools::default(), "zig", VerifyKind::Test, None).is_err());
}

#[test]
fn each_linter_has_its_fix_mode_and_cpp_lints_with_clang_tidy() {
    let mut tools = ProjectTools::default();
    let py = fix_command(&tools, "python").unwrap().unwrap().join(" ");
    assert_eq!(py, "ruff check . --fix --output-format concise");
    assert!(fix_command(&tools, "go").unwrap().is_none());
    assert!(fix_command(&tools, "rust").unwrap().is_none());
    assert!(fix_command(&tools, "typescript").unwrap().is_none());
    tools.js_linter = Some("eslint");
    assert!(
        fix_command(&tools, "typescript")
            .unwrap()
            .unwrap()
            .join(" ")
            .ends_with("eslint . --fix -f unix")
    );
    tools.js_linter = Some("biome");
    assert!(
        fix_command(&tools, "typescript")
            .unwrap()
            .unwrap()
            .join(" ")
            .ends_with("biome lint --write .")
    );
    let cpp = fix_command(&tools, "cpp").unwrap().unwrap().join(" ");
    assert!(cpp.contains("clang-tidy -p build --quiet -fix"), "{cpp}");
    let lint = plan_command_with(&tools, "cpp", VerifyKind::Lint, None)
        .unwrap()
        .join(" ");
    assert!(
        lint.contains("-DCMAKE_EXPORT_COMPILE_COMMANDS=ON") && lint.ends_with("--quiet"),
        "{lint}"
    );
    tools.cpp = CppBuild::Meson;
    assert!(
        plan_command_with(&tools, "cpp", VerifyKind::Lint, None)
            .unwrap()
            .join(" ")
            .contains("meson setup build")
    );
    tools.cpp = CppBuild::Make;
    assert!(plan_command_with(&tools, "cpp", VerifyKind::Lint, None).is_err());
}

/// Go is linted with golangci-lint when the node has it and with `go vet` otherwise, and a
/// project with a golangci config gets golangci-lint's fix mode (#400).
#[test]
fn go_lints_with_golangci_lint_and_falls_back_to_go_vet() {
    let lint = plan_command("go", VerifyKind::Lint, None).unwrap();
    assert_eq!(lint[..2], ["sh", "-c"]);
    assert_eq!(lint[3..], ["sh", "./..."]);
    let script = &lint[2];
    assert!(
        script.contains("command -v golangci-lint")
            && script.contains("exec golangci-lint run \"$@\"")
            && script.contains("exec go vet \"$@\""),
        "{script}"
    );

    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("go.mod"), "module m\n").unwrap();
    let plain = detect_tools(temp.path());
    assert!(!plain.golangci_config);
    assert!(fix_command(&plain, "go").unwrap().is_none());
    std::fs::write(temp.path().join(".golangci.yml"), "version: \"2\"\n").unwrap();
    let configured = detect_tools(temp.path());
    assert!(configured.golangci_config);
    assert_eq!(
        fix_command(&configured, "go").unwrap().unwrap(),
        ["golangci-lint", "run", "--fix", "./..."]
    );

    // The fallback's line on stderr becomes a note at the head of the report.
    let notes = script_notes(
        "prod-code: golangci-lint is not installed on this node; linting with go vet\n# m\n",
    );
    assert_eq!(notes.len(), 1);
    assert_eq!(
        notes[0].render(),
        "note: golangci-lint is not installed on this node; linting with go vet"
    );
}
