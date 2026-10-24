/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */
use super::*;

#[test]
fn coverage_runner_dispatch() {
    for command in [
        vec!["/usr/bin/cargo", "test"], vec!["cargo", "nextest"],
        vec!["go", "test", "-json"], vec!["pytest"], vec!["py.test"],
        vec!["python", "-m", "pytest"], vec!["python3", "-m", "unittest"],
        vec!["vitest"], vec!["jest"], vec!["bun", "test"],
        vec!["npm", "exec", "vitest"], vec!["npx", "jest"],
        vec!["pnpm", "vitest"], vec!["yarn", "jest"], vec!["swift", "test"],
        vec!["xcodebuild"], vec!["ctest"], vec!["meson", "test"],
    ] {
        let command: Vec<_> = command.into_iter().map(str::to_string).collect();
        assert_eq!(test_counts(&command, ""), Some((0, 0)), "{command:?}");
    }
    for command in [vec![], vec!["cargo", "check"], vec!["go", "test"], vec!["bun", "run"], vec!["python3", "main.py"], vec!["meson", "setup"]] {
        let command: Vec<_> = command.into_iter().map(str::to_string).collect();
        assert_eq!(test_counts(&command, ""), None, "{command:?}");
    }
    assert_eq!(test_counts(&["cargo".into(), "test".into()], "test result: ok. 7 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out"), Some((7, 2)));
}

#[test]
fn coverage_ranking_priorities_and_stable_ties() {
    let make = |exit, tests, changed_lines| HypothesisOutcome {
        name: "candidate".into(), exit_code: Some(exit), duration_ms: 0,
        timed_out: false, error: None, output: String::new(), output_len: 0,
        tests, diff: String::new(), changed_lines,
    };
    let mut results = vec![
        make(1, Some((9, 1)), 1), make(0, Some((2, 0)), 2),
        make(0, Some((3, 0)), 5), make(0, Some((3, 0)), 1),
        make(0, Some((3, 0)), 1), make(1, Some((20, 2)), 0),
        make(0, None, 0),
    ];
    assert_eq!(rank(&results), [3, 4, 2, 1, 6, 0, 5]);
    results[3].timed_out = true;
    results[4].error = Some("refused".into());
    assert_eq!(rank(&results), [2, 1, 6, 3, 4, 0, 5]);
    assert_eq!(rank(&[]), Vec::<usize>::new());
}
