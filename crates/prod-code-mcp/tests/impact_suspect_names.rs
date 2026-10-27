/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! #1472: a failing test's name, as the runner's parser reports it, finds the changed
//! functions that reach the analyzer's test of that name. unittest, Jest and Vitest decorate
//! the name; the decoration is not the test.

use prod_code_mcp::impact::{Reach, Symbol, suspects_for};
use prod_code_mcp::verify::parse::js_ts::{parse_jest_text, parse_vitest_text};
use prod_code_mcp::verify::parse::python::parse_unittest_text;

fn sym(name: &str, file: &str) -> Symbol {
    Symbol {
        name: name.to_string(),
        file: file.to_string(),
        line: 1,
        col: 1,
    }
}

fn reaches(test: &str, file: &str) -> Vec<Reach> {
    vec![Reach {
        test: sym(test, file),
        changed: sym("add", "src/math"),
        hops: 1,
    }]
}

/// The one failure `parse` finds in `output`.
fn failed(
    parse: fn(&str) -> (u64, u64, Vec<prod_code_mcp::verify::TestFailure>),
    output: &str,
) -> String {
    let (_, _, failures) = parse(output);
    assert_eq!(failures.len(), 1, "{output}");
    failures[0].name.clone()
}

#[test]
fn decorated_runner_names_find_their_suspects() {
    let unittest = "test_add (test_math.MathTests.test_add) ... FAIL\n\n======================================================================\nFAIL: test_add (test_math.MathTests.test_add)\n----------------------------------------------------------------------\nAssertionError: 3 != 4\n\n----------------------------------------------------------------------\nRan 1 test in 0.001s\n";
    // Python before 3.11 names the class only.
    let unittest_old = unittest.replace("(test_math.MathTests.test_add)", "(test_math.MathTests)");
    let jest = "  ● math › doubles\n\n    expect(received).toBe(expected)\n\nTests:       1 failed, 1 total\n";
    let vitest = " FAIL  src/math.test.ts > math > doubles\nAssertionError: expected 3 to be 4\n Test Files  1 failed (1)\n      Tests  1 failed (1)\n";
    for (runner, name, test, file) in [
        (
            "unittest",
            failed(parse_unittest_text, unittest),
            "test_add",
            "test_math.py",
        ),
        (
            "unittest < 3.11",
            failed(parse_unittest_text, &unittest_old),
            "test_add",
            "test_math.py",
        ),
        (
            "jest",
            failed(parse_jest_text, jest),
            "doubles",
            "src/math.test.ts",
        ),
        (
            "vitest",
            failed(parse_vitest_text, vitest),
            "doubles",
            "src/math.test.ts",
        ),
    ] {
        let found = suspects_for(&reaches(test, file), &name);
        assert_eq!(found.len(), 1, "{runner}: `{name}` found no suspect");
    }
    // The leaf of another test is not this one.
    assert!(suspects_for(&reaches("halves", "src/math.test.ts"), "math > doubles").is_empty());
}
