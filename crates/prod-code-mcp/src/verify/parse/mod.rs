/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod bench;
pub mod common;
pub mod cpp;
pub mod go;
pub mod js_ts;
pub mod python;
pub mod rust;
pub mod swift;

pub use bench::*;
pub use common::*;
pub use cpp::*;
pub use go::*;
pub use js_ts::*;
pub use python::*;
pub use rust::*;
pub use swift::*;

use crate::verify::types::{
    BenchResult, CppBuild, Diagnostic, JsTestRunner, ProjectTools, PythonTestRunner, RunEvent,
    TestFailure, VerifyKind,
};

/// The event one line of `language`'s `kind` output carries, when it carries one.
pub fn event_of_line(language: &str, kind: VerifyKind, line: &str) -> Option<RunEvent> {
    match (language, kind) {
        ("rust", VerifyKind::Check | VerifyKind::Lint) => {
            parse_cargo_json_line(line).map(RunEvent::Diagnostic)
        }
        ("rust", VerifyKind::Test) => {
            let rest = line.strip_prefix("test ")?;
            let (name, outcome) = rest.rsplit_once(" ... ")?;
            match outcome.trim() {
                "ok" => Some(RunEvent::Test {
                    name: name.to_string(),
                    ok: true,
                }),
                "FAILED" => Some(RunEvent::Test {
                    name: name.to_string(),
                    ok: false,
                }),
                _ => None,
            }
        }
        ("go", VerifyKind::Test) => {
            let v: serde_json::Value = serde_json::from_str(line).ok()?;
            let name = v.get("Test")?.as_str()?.to_string();
            match v.get("Action")?.as_str()? {
                "pass" => Some(RunEvent::Test { name, ok: true }),
                "fail" => Some(RunEvent::Test { name, ok: false }),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Parses command stdout/stderr into structured diagnostics, fixes, benchmarks, and test results.
#[allow(clippy::type_complexity)]
pub fn parse_verification_output(
    language: &str,
    kind: VerifyKind,
    stdout: &str,
    stderr: &str,
    tools: &ProjectTools,
) -> (
    Vec<Diagnostic>,
    Vec<crate::fixit::Fix>,
    Vec<BenchResult>,
    u64,
    u64,
    Vec<TestFailure>,
) {
    let mut diagnostics = Vec::new();
    let mut fixes = Vec::new();
    let mut benches = Vec::new();
    let (mut tests_passed, mut tests_failed, mut failures) = (0, 0, Vec::new());
    match (language, kind) {
        ("rust", VerifyKind::Check) | ("rust", VerifyKind::Lint) => {
            diagnostics.extend(stdout.lines().filter_map(parse_cargo_json_line));
            fixes.extend(stdout.lines().flat_map(crate::fixit::parse_fixes));
        }
        ("go", VerifyKind::Bench) => {
            diagnostics.extend(parse_go_text(stderr));
            diagnostics.extend(parse_go_text(stdout));
            benches.extend(parse_bench_text(&format!("{stdout}\n{stderr}")));
        }
        (_, VerifyKind::Bench) => {
            diagnostics.extend(parse_rustc_text(stderr));
            benches.extend(parse_bench_text(&format!("{stdout}\n{stderr}")));
        }
        ("rust", VerifyKind::Test) => {
            diagnostics.extend(parse_rustc_text(stderr));
            let (p, f, fails) = parse_cargo_test_text(stdout);
            tests_passed = p;
            tests_failed = f;
            failures = fails;
        }
        ("go", VerifyKind::Test) => {
            diagnostics.extend(parse_go_text(stderr));
            let (p, f, fails) = parse_go_test_json(stdout);
            tests_passed = p;
            tests_failed = f;
            failures = fails;
        }
        ("go", _) => {
            diagnostics.extend(script_notes(stderr));
            diagnostics.extend(parse_go_text(stderr));
            diagnostics.extend(parse_go_text(stdout));
        }
        ("typescript", VerifyKind::Check) => {
            diagnostics.extend(parse_tsc_text(stdout));
            diagnostics.extend(parse_tsc_text(stderr));
        }
        ("python", VerifyKind::Check) => {
            diagnostics.extend(parse_pyright_json(stdout));
        }
        ("python", VerifyKind::Test) => {
            let combined = format!("{stdout}\n{stderr}");
            let (p, f, fails) = match tools.python_tests {
                PythonTestRunner::Pytest => parse_pytest_text(stdout),
                PythonTestRunner::Unittest => parse_unittest_text(&combined),
            };
            tests_passed = p;
            tests_failed = f;
            failures = fails;
        }
        ("typescript", VerifyKind::Test) => {
            let combined = format!("{stdout}\n{stderr}");
            let (p, f, fails) = match tools.js_tests {
                JsTestRunner::Vitest => parse_vitest_text(&combined),
                JsTestRunner::Jest => parse_jest_text(&combined),
                JsTestRunner::BunTest => parse_bun_test_text(&combined),
                JsTestRunner::Mocha | JsTestRunner::Script => (0, 0, Vec::new()),
            };
            tests_passed = p;
            tests_failed = f;
            failures = fails;
        }
        ("swift", VerifyKind::Test) => {
            let combined = format!("{stdout}\n{stderr}");
            // XCTest prints test stdout mixed with build output. Plain `error: ...` lines on
            // stdout are user logs, not compiler diagnostics; compiler/SwiftPM errors remain
            // available on stderr and test failures are parsed from the combined stream below.
            diagnostics.extend(parse_swift_text(stderr));
            diagnostics.retain(|d| !d.message.starts_with("-["));
            let (p, f, fails) = parse_xctest_text(&combined);
            tests_passed = p;
            tests_failed = f;
            failures = fails;
        }
        ("swift", _) => {
            diagnostics.extend(parse_swift_text(stderr));
            diagnostics.extend(parse_swift_text(stdout));
        }
        ("cpp", VerifyKind::Test) => {
            let (p, f, fails) = match tools.cpp {
                CppBuild::CMake => parse_ctest_text(stdout),
                CppBuild::Meson => parse_meson_test_text(stdout),
                CppBuild::Make => (0, 0, Vec::new()),
            };
            tests_passed = p;
            tests_failed = f;
            failures = fails;
        }
        _ => {
            diagnostics.extend(parse_colon_diagnostics(stderr));
            diagnostics.extend(parse_colon_diagnostics(stdout));
        }
    }
    diagnostics.dedup();
    (
        diagnostics,
        fixes,
        benches,
        tests_passed,
        tests_failed,
        failures,
    )
}
