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
fn coverage_conversions_and_ranking() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("scope.rs"), "fn pick(name: &str, path: PathBuf, count: i32, key: ItemId, owned: String, borrowed: &Token, flag: bool) {\n    // target\n}\n").unwrap();
    let report = propose_expressions_in_scope(dir.path(), "scope.rs", 2, " String ").unwrap();
    let compact: Vec<_> = report
        .candidates
        .iter()
        .map(|c| (c.expression.as_str(), c.confidence))
        .collect();
    assert_eq!(
        compact,
        [
            ("owned", 95),
            ("name.to_string()", 90),
            ("path.display().to_string()", 85),
            ("count.to_string()", 75),
            ("flag.to_string()", 75),
            ("key.to_string()", 75),
            ("borrowed.id", 70),
            ("count.id", 70),
            ("flag.id", 70),
            ("key.id", 70),
            ("name.id", 70),
            ("owned.id", 70),
            ("path.id", 70),
            ("borrowed.get_id()", 65),
            ("count.get_id()", 65),
            ("flag.get_id()", 65),
            ("key.get_id()", 65),
            ("name.get_id()", 65),
            ("owned.get_id()", 65),
            ("path.get_id()", 65),
        ]
    );
    assert_eq!(report.candidate_count, compact.len());
    assert_eq!(report.target_type, "String");
    assert_eq!(report.location, "scope.rs:2");
    for (target, expected) in [
        (
            "Token",
            vec![
                ("borrowed", 95),
                ("*borrowed", 85),
                ("borrowed.clone()", 80),
            ],
        ),
        ("&Token", vec![("borrowed", 95)]),
        ("&i32", vec![("count", 95), ("&count", 90)]),
        ("Option<i32>", vec![("Some(count)", 88), ("None", 60)]),
        ("Result<i32, Error>", vec![("Ok(count)", 88)]),
        ("Unknown", vec![]),
    ] {
        let got = propose_expressions_in_scope(dir.path(), "scope.rs", 2, target).unwrap();
        assert_eq!(
            got.candidates
                .iter()
                .map(|c| (c.expression.as_str(), c.confidence))
                .collect::<Vec<_>>(),
            expected,
            "{target}"
        );
    }
    let boolean = propose_expressions_in_scope(dir.path(), "scope.rs", 2, "bool").unwrap();
    assert_eq!(boolean.candidate_count, 15);
    assert_eq!(boolean.candidates[0].expression, "flag");
    assert_eq!(boolean.candidates[1].expression, "!borrowed");
    assert_eq!(
        boolean.candidates.last().unwrap().expression,
        "path.is_empty()"
    );
}

#[test]
fn coverage_scope_extraction_and_rendering() {
    let lines = [
        "pub async fn pick(",
        "    mut count: u32,",
        "    self: Self,",
        "    label: &str,",
        ") {",
        "let mut name = String::new();",
        "let items = vec![1];",
        "let ready = false;",
        "let unknown = 42;",
        "let x: i64 = 3;",
        "let bad name = 1;",
        "// target",
    ];
    assert_eq!(
        extract_in_scope_variables(&lines, 11),
        BTreeMap::from([
            ("count".into(), "u32".into()),
            ("label".into(), "&str".into()),
            ("name".into(), "String".into()),
            ("items".into(), "Vec".into()),
            ("ready".into(), "bool".into()),
            ("unknown".into(), "var".into()),
            ("x".into(), "i64".into()),
        ])
    );
    for t in [
        "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize",
        "f32", "f64", "bool",
    ] {
        assert!(is_primitive(t));
    }
    assert!(!is_primitive("Widget"));
    assert!(types_match(" &&Thing ", "Thing"));
    assert!(!types_match("Thing", "Other"));
    let mut report = ExpressionSynthesisReport {
        target_type: "Thing".into(),
        location: "sample.rs:1".into(),
        candidate_count: 0,
        candidates: vec![],
    };
    let prefix = "⚡ prod-code Type-Directed Expression Synthesis\n────────────────────────────────────────────────────\nTarget Type: `Thing` at sample.rs:1\nCandidates Found: 0\n\n";
    assert_eq!(
        format_expression_synthesis_report(&report),
        format!("{prefix}No in-scope expressions found producing target type.\n")
    );
    report.candidate_count = 1;
    report.candidates.push(ProposedExpression {
        expression: "value".into(),
        source_variable: "value".into(),
        confidence: 95,
        rationale: "Exact match".into(),
    });
    assert_eq!(
        format_expression_synthesis_report(&report),
        format!(
            "{}Ranked Candidate Expressions:\n  1. `value` (Confidence: 95%)\n     └─ Exact match\n",
            prefix.replace("Found: 0", "Found: 1")
        )
    );
    let dir = tempdir().unwrap();
    let missing = dir.path().join("missing.rs");
    assert_eq!(
        propose_expressions_in_scope(dir.path(), missing.to_str().unwrap(), 1, "bool")
            .unwrap_err()
            .to_string(),
        format!("Failed to read file {}", missing.display())
    );
}

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
