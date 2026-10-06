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
fn parses_xctest_output() {
    let text = "Test Case '-[SignalTests.SignalTests testDoubles]' started.\n\
/srv/ws/Tests/SignalTests/SignalTests.swift:6: error: -[SignalTests.SignalTests testDoubles] : XCTAssertEqual failed: (\"42\") is not equal to (\"43\")\n\
Test Case '-[SignalTests.SignalTests testDoubles]' failed (0.411 seconds).\n\
Test Case 'OtherTests.testOk' passed (0.001 seconds).\n\
\t Executed 2 tests, with 1 failure (0 unexpected) in 0.4 (0.4) seconds\n\
✔ Test run with 0 tests in 0 suites passed after 0.001 seconds.\n";
    let (passed, failed, failures) = parse_xctest_text(text);
    assert_eq!((passed, failed), (1, 1));
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].name, "SignalTests.SignalTests.testDoubles");
    assert!(
        failures[0]
            .output
            .contains("SignalTests.swift:6: XCTAssertEqual failed")
    );
}

#[test]
fn parses_swift_output_compiler_and_git_shell_errors() {
    let output = "\
/srv/ws/Sources/App/main.swift:10:5: error: cannot find 'foo' in scope\n\
Fetching git@github.com:apple/swift-argument-parser.git\n\
error: 'swift-argument-parser': GitShellError(result: <ProcessResult: exit: terminated(code: 128), output:>)\n";
    let diags = parse_swift_text(output);
    assert_eq!(diags.len(), 2);
    assert_eq!(
        diags[0].file.as_deref(),
        Some("/srv/ws/Sources/App/main.swift")
    );
    assert_eq!(diags[0].line, Some(10));
    assert_eq!(diags[0].column, Some(5));
    assert_eq!(diags[0].message, "cannot find 'foo' in scope");

    assert_eq!(diags[1].file.as_deref(), Some("Package.swift"));
    assert_eq!(diags[1].code.as_deref(), Some("git-fetch"));
    assert!(diags[1].message.contains("Git SSH authentication failed"));
    assert!(
        diags[1]
            .message
            .contains("git@github.com:apple/swift-argument-parser.git")
    );
}

#[test]
fn parses_swift_git_shell_error_with_output() {
    let output = "\
Fetching https://github.com/foo/bar.git\n\
error: 'bar': GitShellError(result: <ProcessResult: exit: terminated(code: 128), output:fatal: could not read Username for 'https://github.com': terminal prompts disabled>)\n";
    let diags = parse_swift_text(output);
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].file.as_deref(), Some("Package.swift"));
    assert_eq!(diags[0].code.as_deref(), Some("git-fetch"));
    assert!(diags[0].message.contains("fatal: could not read Username"));
}

#[test]
fn parses_swift_spm_package_manifest_error() {
    let output = "error: 'foo': package 'foo' @ 1.0.0 is using Swift tools version 5.9.0 but the installed toolchain is 5.8.0\n";
    let diags = parse_swift_text(output);
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].file.as_deref(), Some("Package.swift"));
    assert_eq!(
        diags[0].message,
        "'foo': package 'foo' @ 1.0.0 is using Swift tools version 5.9.0 but the installed toolchain is 5.8.0"
    );
}

#[test]
fn parses_ctest_output() {
    let text = "Test project /srv/ws/build\n\
Start 1: adds\n\
1/2 Test #1: adds .............................   Passed    0.01 sec\n\
Start 2: fails\n\
2/2 Test #2: fails ............................***Failed    0.02 sec\n\
expected 42, got 43\n\
\n\
50% tests passed, 1 tests failed out of 2\n";
    let (passed, failed, failures) = parse_ctest_text(text);
    assert_eq!((passed, failed), (1, 1));
    assert_eq!(failures[0].name, "fails");
    assert!(failures[0].output.contains("expected 42, got 43"));
}

#[test]
fn relativizes_server_paths() {
    let mut diagnostics = vec![Diagnostic {
        level: "error".into(),
        code: None,
        message: "boom".into(),
        file: Some("/srv/ws/src/a.cpp".into()),
        line: Some(1),
        column: None,
    }];
    relativize_diagnostics(&mut diagnostics, "/srv/ws");
    assert_eq!(diagnostics[0].file.as_deref(), Some("src/a.cpp"));
}

#[test]
fn cargo_json_diagnostic() {
    let line = r#"{"reason":"compiler-message","message":{"level":"error","code":{"code":"E0425"},"message":"cannot find value `x` in this scope","spans":[{"file_name":"src/lib.rs","line_start":3,"column_start":9,"is_primary":true}]}}"#;
    let d = parse_cargo_json_line(line).unwrap();
    assert_eq!(
        d.render(),
        "error: [E0425] cannot find value `x` in this scope (src/lib.rs:3:9)"
    );
    let summary = r#"{"reason":"compiler-message","message":{"level":"warning","message":"2 warnings emitted","spans":[]}}"#;
    assert!(parse_cargo_json_line(summary).is_none());
    assert!(parse_cargo_json_line(r#"{"reason":"build-finished","success":true}"#).is_none());
}

#[test]
fn rustc_text_diagnostics() {
    let text = "error[E0308]: mismatched types\n  --> crates/a/src/lib.rs:12:5\n   |\nwarning: unused variable: `y`\n --> src/main.rs:4:9\nerror: aborting due to 1 previous error\n";
    let d = parse_rustc_text(text);
    assert_eq!(d.len(), 2);
    assert_eq!(d[0].code.as_deref(), Some("E0308"));
    assert_eq!(d[0].file.as_deref(), Some("crates/a/src/lib.rs"));
    assert_eq!((d[0].line, d[0].column), (Some(12), Some(5)));
    assert_eq!(d[1].level, "warning");
    assert_eq!(d[1].file.as_deref(), Some("src/main.rs"));
}

#[test]
fn cargo_test_text() {
    let text = "running 3 tests\ntest a::ok ... ok\ntest a::bad ... FAILED\ntest a::also ... ok\n\nfailures:\n\n---- a::bad stdout ----\nthread 'a::bad' panicked at src/lib.rs:5:9:\nassertion failed: 1 == 2\n\n\nfailures:\n    a::bad\n\ntest result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n";
    let (p, f, fails) = parse_cargo_test_text(text);
    assert_eq!((p, f), (2, 1));
    assert_eq!(fails.len(), 1);
    assert_eq!(fails[0].name, "a::bad");
    assert!(fails[0].output.contains("assertion failed: 1 == 2"));
}

#[test]
fn go_text_and_json() {
    let d = parse_go_text("# prod/cmd\ncmd/main.go:10:2: undefined: foo\n");
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].render(), "error: undefined: foo (cmd/main.go:10:2)");
    // golangci-lint quotes the source line under each finding and ends with a summary.
    let golangci = "main.go:9:10: Error return value of `os.Remove` is not checked (errcheck)\n\tos.Remove(\"x\")\n\t         ^\ncalc/calc.go:4:2: ineffectual assignment to total (ineffassign)\n\ttotal := 0\n\t^\n2 issues:\n* errcheck: 1\n* ineffassign: 1\n";
    let d = parse_go_text(golangci);
    assert_eq!(d.len(), 2, "{d:?}");
    assert_eq!(
        d[0].render(),
        "error: Error return value of `os.Remove` is not checked (errcheck) (main.go:9:10)"
    );
    assert_eq!(d[1].file.as_deref(), Some("calc/calc.go"));
    let events = r#"{"Action":"run","Package":"p","Test":"TestA"}
{"Action":"output","Package":"p","Test":"TestA","Output":"    a_test.go:7: boom\n"}
{"Action":"fail","Package":"p","Test":"TestA","Elapsed":0}
{"Action":"pass","Package":"p","Test":"TestB","Elapsed":0}
{"Action":"fail","Package":"p","Elapsed":0.1}"#;
    let (p, f, fails) = parse_go_test_json(events);
    assert_eq!((p, f), (1, 1));
    assert_eq!(fails[0].name, "p.TestA");
    assert!(fails[0].output.contains("boom"));
}

#[test]
fn go_package_level_compilation_failure_without_test_field() {
    let events = r##"{"Action":"output","Package":"example.com/pkg","Output":"# example.com/pkg\n"}
{"Action":"output","Package":"example.com/pkg","Output":"pkg/foo.go:10:2: undefined: bar\n"}
{"Action":"fail","Package":"example.com/pkg","Elapsed":0.005}"##;
    let (p, f, fails) = parse_go_test_json(events);
    assert_eq!((p, f), (0, 1));
    assert_eq!(fails.len(), 1);
    assert_eq!(fails[0].name, "example.com/pkg");
    assert!(fails[0].output.contains("undefined: bar"));

    // When a package has individual test failures, the package-level fail
    // event does not double-count or overwrite the test failure.
    let with_test_fail = r#"{"Action":"run","Package":"example.com/pkg","Test":"TestX"}
{"Action":"output","Package":"example.com/pkg","Test":"TestX","Output":"    x_test.go:5: fail\n"}
{"Action":"fail","Package":"example.com/pkg","Test":"TestX","Elapsed":0}
{"Action":"fail","Package":"example.com/pkg","Elapsed":0.01}"#;
    let (p, f, fails) = parse_go_test_json(with_test_fail);
    assert_eq!((p, f), (0, 1));
    assert_eq!(fails.len(), 1);
    assert_eq!(fails[0].name, "example.com/pkg.TestX");
    assert!(fails[0].output.contains("x_test.go:5: fail"));
}

#[test]
fn go_benchmark_compiler_diagnostics() {
    let tools = ProjectTools::default();
    let stderr = "# example.com/bench\nbench_test.go:14:2: undefined: nonExistentFunc\n";
    let (diags, fixes, benches, p, f, fails) =
        parse_verification_output("go", VerifyKind::Bench, "", stderr, &tools);
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].file.as_deref(), Some("bench_test.go"));
    assert_eq!(diags[0].line, Some(14));
    assert_eq!(diags[0].column, Some(2));
    assert_eq!(diags[0].message, "undefined: nonExistentFunc");
    assert!(fixes.is_empty());
    assert!(benches.is_empty());
    assert_eq!((p, f), (0, 0));
    assert!(fails.is_empty());

    let stdout = "bench_test.go:20:5: syntax error: unexpected semicolon\n";
    let (diags, _fixes, _benches, _p, _f, _fails) =
        parse_verification_output("go", VerifyKind::Bench, stdout, "", &tools);
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].file.as_deref(), Some("bench_test.go"));
    assert_eq!(diags[0].line, Some(20));
    assert_eq!(diags[0].column, Some(5));
    assert_eq!(diags[0].message, "syntax error: unexpected semicolon");
}

#[test]
fn colon_tsc_pyright_pytest_parsers() {
    let d = parse_colon_diagnostics(
        "src/a.cpp:12:5: error: no member named 'x'\nsrc/b.cpp:3:1: warning: unused\nnote: ignored\n",
    );
    assert_eq!(d.len(), 2);
    assert_eq!(d[0].render(), "error: no member named 'x' (src/a.cpp:12:5)");
    assert_eq!(d[1].level, "warning");
    let t = parse_tsc_text(
        "src/index.ts(7,3): error TS2322: Type 'string' is not assignable to type 'number'.\n",
    );
    assert_eq!(t.len(), 1);
    assert_eq!(
        t[0].render(),
        "error: [TS2322] Type 'string' is not assignable to type 'number'. (src/index.ts:7:3)"
    );
    let py = parse_pyright_json(
        r#"{"generalDiagnostics":[{"file":"/w/a.py","severity":"error","message":"boom","range":{"start":{"line":4,"character":2}},"rule":"reportGeneralTypeIssues"}],"summary":{}}"#,
    );
    assert_eq!(
        py[0].render(),
        "error: [reportGeneralTypeIssues] boom (/w/a.py:5:3)"
    );
    let (p, f, fails) = parse_pytest_text(
        "FAILED tests/test_a.py::test_x - AssertionError: nope\n===== 1 failed, 3 passed in 0.10s =====\n",
    );
    assert_eq!((p, f), (3, 1));
    assert_eq!(fails[0].name, "tests/test_a.py::test_x");
    assert!(
        plan_command("swift", VerifyKind::Test, Some("Foo"))
            .unwrap()
            .ends_with(&["--filter".to_string(), "Foo".to_string()])
    );
    // C++ lints with clang-tidy since #205.
    assert!(
        plan_command("cpp", VerifyKind::Lint, None)
            .unwrap()
            .join(" ")
            .contains("clang-tidy -p build")
    );
}

#[test]
fn benchmark_results_are_read_from_criterion_libtest_and_go() {
    let criterion = "Benchmarking sum 1000\nBenchmarking sum 1000: Warming up for 3.0000 s\nsum 1000                time:   [2.3499 ns 2.3500 ns 2.3502 ns]\na_benchmark_with_a_very_long_name\n                        time:   [10.1 µs 10.2 µs 10.4 µs]\n                        change: [-1.0% +0.5% +2.0%]\n";
    let libtest = "test sort::big ... bench:       1,234 ns/iter (+/- 56)\n";
    let go = "BenchmarkSum-8   \t 1000000\t      1234 ns/op\nPASS\n";
    let found = parse_bench_text(&format!("{criterion}{libtest}{go}"));
    let rows: Vec<(&str, &str, Option<&str>)> = found
        .iter()
        .map(|b| (b.name.as_str(), b.estimate.as_str(), b.range.as_deref()))
        .collect();
    assert_eq!(
        rows,
        [
            ("sum 1000", "2.3500 ns", Some("2.3499 ns .. 2.3502 ns")),
            (
                "a_benchmark_with_a_very_long_name",
                "10.2 µs",
                Some("10.1 µs .. 10.4 µs")
            ),
            ("sort::big", "1234 ns/iter", Some("+/- 56")),
            ("BenchmarkSum-8", "1234 ns/op", None),
        ]
    );
    assert_eq!(
        plan_command_basic("go", VerifyKind::Bench, None).unwrap(),
        ["go", "test", "-run", "^$", "-bench", ".", "./..."]
    );
    assert_eq!(
        plan_command_basic("rust", VerifyKind::Bench, Some("sum")).unwrap(),
        ["cargo", "bench", "--workspace", "sum"]
    );
    let report = VerifyReport {
        kind: VerifyKind::Bench,
        language: "rust".into(),
        command: vec!["cargo".into(), "bench".into()],
        exit_code: Some(0),
        timed_out: false,
        duration_ms: 9000,
        diagnostics: vec![],
        tests_passed: 0,
        tests_failed: 0,
        failures: vec![],
        tail: String::new(),
        fixes: vec![],
        benches: found,
        usage: None,
        platform: None,
    };
    let text = report.render(10);
    assert!(
        text.contains("rust bench: OK in 9.0s; 4 benchmark(s)"),
        "{text}"
    );
    assert!(
        text.contains("  sum 1000  2.3500 ns  [2.3499 ns .. 2.3502 ns]"),
        "{text}"
    );
}
