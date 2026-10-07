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
use tempfile::tempdir;

#[test]
fn test_expression_synthesis() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("main.rs");
    let code = r#"
fn test_handler(user_id: u64, name: &str, is_admin: bool) {
    let extra_token = "secret";
    // Target line:
    let output = 42;
}
"#;
    std::fs::write(&file, code).unwrap();

    let report = propose_expressions_in_scope(dir.path(), "main.rs", 5, "u64").unwrap();
    assert!(!report.candidates.is_empty());
    assert_eq!(report.candidates[0].expression, "user_id");

    let report_str = propose_expressions_in_scope(dir.path(), "main.rs", 5, "String").unwrap();
    let has_to_string = report_str
        .candidates
        .iter()
        .any(|c| c.expression == "name.to_string()");
    assert!(has_to_string);
}

#[test]
fn test_expression_synthesis_multiline_header_at_target_line() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("main.rs");
    let code = "fn test_multiline(\n    user_id: u64,\n    name: &str,\n) {\n}\n";
    std::fs::write(&file, code).unwrap();

    // Target line 1 is the function header itself, which lacks closing paren on line 1
    let report = propose_expressions_in_scope(dir.path(), "main.rs", 1, "u64").unwrap();
    assert!(report.candidates.is_empty());
}
