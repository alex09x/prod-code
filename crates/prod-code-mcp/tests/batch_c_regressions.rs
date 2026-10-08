/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::diagnostics::{
    DiagnosticsReport, DocDiagnostic, is_unresolved_prelude_macro, set_aside_derive_expansions,
};
use prod_code_mcp::extract_function::binding::uncaptured_enclosing_locals;
use prod_code_mcp::tools::symbols::alias::extract_alias_target;
use prod_code_mcp::tools::symbols::sources::find_identifier_on_line;

#[test]
fn test_find_identifier_on_line_swift_offsets() {
    let row = "    override open func keyDown(with event: NSEvent) {";
    let col = find_identifier_on_line(row, "keyDown");
    assert_eq!(col, Some(23), "keyDown starts at index 23");

    let row2 = "    public func setScoped(_ scoped: Bool) {";
    let col2 = find_identifier_on_line(row2, "setScoped");
    assert_eq!(col2, Some(16), "setScoped starts at index 16");

    let row_negative = "    let keyDownward = true";
    assert_eq!(find_identifier_on_line(row_negative, "keyDown"), None);
}

#[test]
fn test_unresolved_prelude_macros_filter() {
    assert!(is_unresolved_prelude_macro("unresolved macro assert_eq!"));
    assert!(is_unresolved_prelude_macro("unresolved macro `assert_eq!`"));
    assert!(is_unresolved_prelude_macro(
        "cannot find macro `vec!` in this scope"
    ));
    assert!(is_unresolved_prelude_macro("unresolved macro `format!`"));
    assert!(!is_unresolved_prelude_macro(
        "unresolved macro `my_custom_macro!`"
    ));

    let mut report = DiagnosticsReport {
        file: "crates/prod-code-gateway/src/metrics/tests.rs".to_string(),
        errors: 2,
        warnings: 0,
        items: vec![
            DocDiagnostic {
                severity: "error".to_string(),
                code: Some("unresolved-macro-call".to_string()),
                message: "unresolved macro assert_eq!".to_string(),
                line: 10,
                col: 5,
                source: Some("rust-analyzer".to_string()),
                note: None,
                end: None,
            },
            DocDiagnostic {
                severity: "error".to_string(),
                code: Some("unresolved-macro-call".to_string()),
                message: "unresolved macro vec!".to_string(),
                line: 15,
                col: 5,
                source: Some("rust-analyzer".to_string()),
                note: None,
                end: None,
            },
        ],
        preexisting: Vec::new(),
        in_derive: Vec::new(),
        auto_trait: Vec::new(),
        hallucinations: Vec::new(),
    };

    set_aside_derive_expansions(&mut report, "fn test() { assert_eq!(1, 1); vec![1]; }");
    assert_eq!(
        report.errors, 0,
        "Prelude macros should not count as errors"
    );
    assert_eq!(report.auto_trait.len(), 2);
    assert!(
        report.ok(),
        "Report should be clean with prelude macros set aside"
    );
    assert!(
        report
            .render()
            .contains("analyzer limitation(s) are not counted")
    );
}

#[test]
fn test_extract_type_alias_target() {
    let code = "pub use super::agent_tracker::{AgentOwners, AgentTracker as IdentityState};";
    let target = extract_alias_target(code, "IdentityState");
    assert_eq!(target.as_deref(), Some("AgentTracker"));

    let code2 = "type IdentityState = super::agent_tracker::AgentTracker;";
    let target2 = extract_alias_target(code2, "IdentityState");
    assert_eq!(target2.as_deref(), Some("AgentTracker"));

    let code3 = "pub type ClientContext = ClientContextImpl<DefaultConfig>;";
    let target3 = extract_alias_target(code3, "ClientContext");
    assert_eq!(target3.as_deref(), Some("ClientContextImpl"));
}

#[test]
fn test_uncaptured_enclosing_locals_detected() {
    let original = r#"
fn format_slice(function_name: &str, file_rel: &str) -> String {
    let start_line = 10;
    let end_line = 20;
    let target_line = 15;
    let output = format!("{function_name} in {file_rel}:{start_line}-{end_line} target {target_line}");
    output
}
"#;
    let start = original.find("let output = format!").unwrap();
    // Simulate extracted function where function_name and file_rel were omitted from parameters
    let extracted = r#"
fn format_slice(function_name: &str, file_rel: &str) -> String {
    let start_line = 10;
    let end_line = 20;
    let target_line = 15;
    let output = fun_name(start_line, end_line);
    output
}

fn fun_name(start_line: usize, end_line: usize) -> String {
    format!("{function_name} in {file_rel}:{start_line}-{end_line} target {target_line}")
}
"#;
    let uncaptured = uncaptured_enclosing_locals(original, start, extracted, None);
    assert!(
        uncaptured.contains(&"function_name".to_string()),
        "Expected function_name to be flagged as uncaptured: {uncaptured:?}"
    );
    assert!(
        uncaptured.contains(&"file_rel".to_string()),
        "Expected file_rel to be flagged as uncaptured: {uncaptured:?}"
    );
    assert!(
        uncaptured.contains(&"target_line".to_string()),
        "Expected target_line to be flagged as uncaptured: {uncaptured:?}"
    );
    assert!(
        !uncaptured.contains(&"start_line".to_string()),
        "start_line was declared as parameter, must not be flagged"
    );
    assert!(
        !uncaptured.contains(&"end_line".to_string()),
        "end_line was declared as parameter, must not be flagged"
    );
}
