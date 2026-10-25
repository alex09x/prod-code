/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::verify::parse::*;
use crate::verify::plan::*;
use crate::verify::types::*;

#[test]
fn wave2_python_and_gradle_plans() {
    for (runtime, python, check, lint) in [
        (
            PythonRuntime::System,
            vec!["python3"],
            vec!["basedpyright", "--outputjson"],
            vec!["ruff"],
        ),
        (
            PythonRuntime::Uv,
            vec!["uv", "run", "python"],
            vec!["uv", "run", "basedpyright", "--outputjson"],
            vec!["uv", "run", "ruff"],
        ),
        (
            PythonRuntime::Venv(".venv/bin/python".into()),
            vec![".venv/bin/python"],
            vec![
                "basedpyright",
                "--outputjson",
                "--pythonpath",
                ".venv/bin/python",
            ],
            vec!["ruff"],
        ),
    ] {
        for (runner, args) in [
            (PythonTestRunner::Pytest, vec!["-m", "pytest", "-q", "-rf"]),
            (PythonTestRunner::Unittest, vec!["-m", "unittest", "-v"]),
        ] {
            let tools = ProjectTools {
                python: runtime.clone(),
                python_tests: runner,
                ..Default::default()
            };
            assert_eq!(
                plan_command_with(&tools, "python", VerifyKind::Check, None).unwrap(),
                check
            );
            let mut expected = lint.clone();
            expected.extend(["check", ".", "--output-format", "concise"]);
            assert_eq!(
                plan_command_with(&tools, "python", VerifyKind::Lint, None).unwrap(),
                expected
            );
            for filter in [None, Some("sample")] {
                let mut expected = python.clone();
                expected.extend(&args);
                if let Some(f) = filter {
                    expected.extend(["-k", f]);
                }
                assert_eq!(
                    plan_command_with(&tools, "python", VerifyKind::Test, filter).unwrap(),
                    expected
                );
            }
        }
    }
    for (wrapper, binary) in [(true, "./gradlew"), (false, "gradle")] {
        let tools = ProjectTools {
            java_gradle: true,
            java_gradle_wrapper: wrapper,
            ..Default::default()
        };
        assert_eq!(
            plan_command_with(&tools, "java", VerifyKind::Check, None).unwrap(),
            [binary, "testClasses"]
        );
        assert_eq!(
            plan_command_with(&tools, "java", VerifyKind::Test, None).unwrap(),
            [binary, "test"]
        );
        assert_eq!(
            plan_command_with(&tools, "java", VerifyKind::Test, Some("Suite.*")).unwrap(),
            [binary, "test", "--tests", "Suite.*"]
        );
    }
}

#[test]
fn wave2_cpp_and_zig_plans() {
    for (build, check, test) in [
        (
            CppBuild::CMake,
            vec![
                "sh",
                "-c",
                "cmake -S . -B build -DCMAKE_EXPORT_COMPILE_COMMANDS=ON >/dev/null && cmake --build build",
            ],
            vec![
                "sh",
                "-c",
                "cmake --build build >/dev/null && ctest --test-dir build --output-on-failure",
            ],
        ),
        (
            CppBuild::Meson,
            vec![
                "sh",
                "-c",
                "[ -d build ] || meson setup build >/dev/null; meson compile -C build",
            ],
            vec!["meson", "test", "-C", "build", "--print-errorlogs"],
        ),
        (CppBuild::Make, vec!["make"], vec!["make", "test"]),
    ] {
        let tools = ProjectTools {
            cpp: build,
            ..Default::default()
        };
        assert_eq!(
            plan_command_with(&tools, "cpp", VerifyKind::Check, None).unwrap(),
            check
        );
        assert_eq!(
            plan_command_with(&tools, "cpp", VerifyKind::Test, None).unwrap(),
            test
        );
        let expected = match build {
            CppBuild::CMake => vec![
                "sh",
                "-c",
                "cmake --build build >/dev/null && ctest --test-dir build --output-on-failure -R 'it'\\''s'",
            ],
            CppBuild::Meson => vec!["meson", "test", "-C", "build", "--print-errorlogs", "it's"],
            CppBuild::Make => vec!["make", "test"],
        };
        assert_eq!(
            plan_command_with(&tools, "cpp", VerifyKind::Test, Some("it's")).unwrap(),
            expected
        );
    }
    assert_eq!(
        plan_command("zig", VerifyKind::Test, None)
            .unwrap_err()
            .to_string(),
        "Zig test planning needs a build.zig test step or an explicit source target"
    );
    let tools = ProjectTools {
        zig_test_step: true,
        ..Default::default()
    };
    assert_eq!(
        plan_command_with(&tools, "zig", VerifyKind::Test, Some("")).unwrap(),
        ["zig", "build", "test"]
    );
    assert_eq!(
        plan_command_with(&tools, "zig", VerifyKind::Test, Some("sample"))
            .unwrap_err()
            .to_string(),
        "Zig build test steps do not support the generic test-name filter; rerun without a filter"
    );
}

#[test]
fn wave2_parser_dispatch_counts_and_failures() {
    for (language, tools, stdout, stderr, passed, failed, name, output) in [
        (
            "python",
            ProjectTools::default(),
            "FAILED sample.py::test_value - mismatch\n1 passed, 1 failed",
            "",
            1,
            1,
            "sample.py::test_value",
            "mismatch",
        ),
        (
            "python",
            ProjectTools {
                python_tests: PythonTestRunner::Unittest,
                ..Default::default()
            },
            "test_ok (suite) ... ok\n",
            "test_bad (suite) ... FAIL\nFAIL: test_bad (suite)\nAssertionError: mismatch\nRan 2 tests",
            1,
            1,
            "test_bad (suite)",
            "AssertionError: mismatch",
        ),
        (
            "typescript",
            ProjectTools {
                js_tests: JsTestRunner::Jest,
                ..Default::default()
            },
            "",
            "  ● sample\nmismatch\nTests: 1 failed, 2 passed, 3 total",
            2,
            1,
            "sample",
            "mismatch",
        ),
        (
            "typescript",
            ProjectTools {
                js_tests: JsTestRunner::Vitest,
                ..Default::default()
            },
            " FAIL  sample\nmismatch\n",
            " Tests  1 failed | 2 passed (3)",
            2,
            1,
            "sample",
            "mismatch",
        ),
        (
            "typescript",
            ProjectTools {
                js_tests: JsTestRunner::BunTest,
                ..Default::default()
            },
            "(fail) sample [1ms]\nmismatch\n",
            " 2 pass\n 1 fail",
            2,
            1,
            "sample",
            "mismatch",
        ),
        (
            "cpp",
            ProjectTools {
                cpp: CppBuild::Meson,
                ..Default::default()
            },
            "1/2 good OK 0.01s\n2/2 sample FAIL 0.02s",
            "",
            1,
            1,
            "sample",
            "sample FAIL 0.02s",
        ),
        (
            "cpp",
            ProjectTools::default(),
            "1/2 Test #1: good .... Passed 0.01 sec\n2/2 Test #2: sample ....***Failed 0.02 sec\nmismatch\n50% tests passed, 1 tests failed out of 2",
            "",
            1,
            1,
            "sample",
            "mismatch",
        ),
    ] {
        let (diagnostics, fixes, benches, p, f, failures) =
            parse_verification_output(language, VerifyKind::Test, stdout, stderr, &tools);
        assert!(diagnostics.is_empty(), "{language}: {diagnostics:?}");
        assert!(fixes.is_empty());
        assert!(benches.is_empty());
        assert_eq!((p, f), (passed, failed), "{language}");
        assert_eq!(failures.len(), 1, "{language}");
        assert_eq!(failures[0].name, name);
        assert_eq!(failures[0].output.trim(), output);
    }
    for tools in [
        ProjectTools {
            js_tests: JsTestRunner::Mocha,
            ..Default::default()
        },
        ProjectTools::default(),
        ProjectTools {
            cpp: CppBuild::Make,
            ..Default::default()
        },
    ] {
        for language in ["typescript", "cpp"] {
            let (d, fixes, benches, p, f, failures) = parse_verification_output(
                language,
                VerifyKind::Test,
                "unstructured output",
                "",
                &tools,
            );
            assert_eq!(
                (d.len(), fixes.len(), benches.len(), p, f, failures.len()),
                (0, 0, 0, 0, 0, 0)
            );
        }
    }
}

#[test]
fn wave2_parser_diagnostics_and_events() {
    let tools = ProjectTools::default();
    let (d, _, _, p, f, failures) = parse_verification_output(
        "python",
        VerifyKind::Check,
        r#"{"generalDiagnostics":[{"severity":"error","message":"missing name","file":"sample.py","range":{"start":{"line":2,"character":3}}}]}"#,
        "ignored",
        &tools,
    );
    assert_eq!(
        d.iter().map(Diagnostic::render).collect::<Vec<_>>(),
        ["error: missing name (sample.py:3:4)"]
    );
    assert_eq!((p, f, failures.len()), (0, 0, 0));
    for (language, kind, stdout, stderr, expected) in [
        (
            "typescript",
            VerifyKind::Check,
            "sample.ts(2,3): error TS2304: missing name",
            "sample.ts(2,3): error TS2304: missing name",
            "error: [TS2304] missing name (sample.ts:2:3)",
        ),
        (
            "go",
            VerifyKind::Lint,
            "",
            "prod-code: using vet",
            "note: using vet",
        ),
        (
            "swift",
            VerifyKind::Check,
            "sample.swift:2:3: error: missing name",
            "",
            "error: missing name (sample.swift:2:3)",
        ),
        (
            "java",
            VerifyKind::Check,
            "sample.java:2:3: error: missing name",
            "",
            "error: missing name (sample.java:2:3)",
        ),
    ] {
        let (d, fixes, benches, p, f, failures) =
            parse_verification_output(language, kind, stdout, stderr, &tools);
        assert_eq!(
            d.iter().map(Diagnostic::render).collect::<Vec<_>>(),
            [expected],
            "{language}"
        );
        assert_eq!(
            (fixes.len(), benches.len(), p, f, failures.len()),
            (0, 0, 0, 0, 0)
        );
    }
    for line in [
        "not json",
        "{}",
        r#"{"Test":2,"Action":"pass"}"#,
        r#"{"Test":"sample"}"#,
        r#"{"Test":"sample","Action":2}"#,
        r#"{"Test":"sample","Action":"output"}"#,
    ] {
        assert_eq!(event_of_line("go", VerifyKind::Test, line), None);
    }
    assert_eq!(
        event_of_line(
            "go",
            VerifyKind::Test,
            r#"{"Test":"sample","Action":"pass"}"#
        ),
        Some(RunEvent::Test {
            name: "sample".into(),
            ok: true
        })
    );
    for line in ["unrelated", "test incomplete", "test sample ... ignored"] {
        assert_eq!(event_of_line("rust", VerifyKind::Test, line), None);
    }
}
