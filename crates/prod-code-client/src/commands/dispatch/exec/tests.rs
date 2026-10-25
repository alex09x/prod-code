/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The refusals `prod-code validate` makes before it reaches the node: option combinations
//! that cannot be honoured and inputs that are missing or unreadable.

use crate::commands::dispatch::tests::refusal;

#[test]
fn validate_refuses_option_combinations_and_missing_inputs() {
    let scratch = tempfile::Builder::new()
        .prefix("validate-inputs")
        .tempdir()
        .expect("scratch");
    let patch = scratch.path().join("missing.patch");
    let proposed = scratch.path().join("missing.rs");
    let patch = patch.to_string_lossy().into_owned();
    let proposed = proposed.to_string_lossy().into_owned();

    let cases: Vec<(Vec<&str>, String)> = vec![
        (
            vec!["validate", "src/a.rs", "--compile", "--json"],
            "--compile reports as text; leave out --json".to_string(),
        ),
        (
            vec!["validate", "src/a.rs", "--borrow-check", "--json"],
            "--borrow-check reports as text; leave out --json".to_string(),
        ),
        (
            vec!["validate", "--chunk", "fn a() {}"],
            "give the file to validate with --chunk".to_string(),
        ),
        (
            vec!["validate", "--stream"],
            "give the file to validate with --stream".to_string(),
        ),
        (
            vec!["validate"],
            "give the file to validate, or `--diff PATCH`".to_string(),
        ),
        (
            vec!["validate", "--diff", &patch],
            format!("failed to read {patch}"),
        ),
        (
            vec!["validate", "src/a.rs", "--from", &proposed],
            format!("failed to read {proposed}"),
        ),
    ];
    for (argv, message) in cases {
        let got = refusal(&argv);
        assert!(got.starts_with(&message), "{argv:?}: {got}");
    }
}
