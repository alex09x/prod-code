/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use tempfile;

use crate::impact::incoming::*;
use crate::impact::test_cmd::*;
use crate::impact::*;

#[test]
fn rust_test_selection_requires_runnable_attributes_even_when_flagged() {
    let root = tempfile::tempdir().unwrap();
    let file = "tests/impact.rs";
    std::fs::create_dir_all(root.path().join("tests")).unwrap();
    std::fs::write(
            root.path().join(file),
            "#[test]\nfn unit() {}\n#[tokio::test]\nasync fn asynchronous() {}\n#[cfg(test)]\nfn cfg_helper() {}\nfn test_named_helper() {}\n",
        )
        .unwrap();
    let classify =
        |name, line, flagged| test_name(root.path(), "rust", name, file, line, flagged).unwrap();

    assert_eq!(classify("unit", 2, false).as_deref(), Some("unit"));
    assert_eq!(classify("unit", 2, true).as_deref(), Some("unit"));
    assert_eq!(
        classify("asynchronous", 4, false).as_deref(),
        Some("asynchronous")
    );
    assert_eq!(
        classify("asynchronous", 4, true).as_deref(),
        Some("asynchronous")
    );
    assert_eq!(classify("cfg_helper", 6, false), None);
    assert_eq!(classify("cfg_helper", 6, true), None);
    assert_eq!(classify("test_named_helper", 7, false), None);
    assert_eq!(classify("test_named_helper", 7, true), None);

    let tests: Vec<Symbol> = [
        ("unit", 2, true),
        ("asynchronous", 4, false),
        ("cfg_helper", 6, true),
        ("test_named_helper", 7, true),
    ]
    .into_iter()
    .filter_map(|(name, line, flagged)| {
        classify(name, line, flagged).map(|name| Symbol {
            name,
            file: file.into(),
            line,
            col: 1,
        })
    })
    .collect();
    let command = test_command("rust", &crate::verify::ProjectTools::default(), &tests)
        .unwrap()
        .join(" ");
    assert!(!command.contains("helper"), "{command}");
}

#[test]
fn rust_conditional_test_attributes_force_whole_suite() {
    let root = tempfile::tempdir().unwrap();
    let file = "tests/conditional.rs";
    std::fs::create_dir_all(root.path().join("tests")).unwrap();
    std::fs::write(
            root.path().join(file),
            "#[cfg_attr(unix, test)]\nfn conditional() {}\n#[cfg_attr(unix, cfg_attr(feature = \"tests\", tokio::test))]\nasync fn conditional_async() {}\n#[cfg_attr(unix, inline)]\nfn flagged_helper() {}\n#[r#test]\nfn raw_named_test() {}\n#[cfg_attr(unix, r#test)]\nfn raw_conditional() {}\n#[cfg_attr(target_os = \"macos\", test)]\n#[test]\nfn direct_test_with_inactive_conditional() {}\n",
        )
        .unwrap();
    assert!(
        test_name(root.path(), "rust", "conditional", file, 2, true).is_err(),
        "conditional test status must not be inferred from the broad analyzer flag"
    );
    assert!(test_name(root.path(), "rust", "conditional", file, 2, false).is_err());
    assert!(test_name(root.path(), "rust", "conditional_async", file, 4, true).is_err());
    assert_eq!(
        test_name(root.path(), "rust", "flagged_helper", file, 6, true).unwrap(),
        None
    );
    assert_eq!(
        test_name(root.path(), "rust", "raw_named_test", file, 8, false)
            .unwrap()
            .as_deref(),
        Some("raw_named_test")
    );
    assert!(test_name(root.path(), "rust", "raw_conditional", file, 10, true).is_err());
    assert!(test_name(root.path(), "rust", "raw_conditional", file, 10, false).is_err());
    assert_eq!(
        test_name(
            root.path(),
            "rust",
            "direct_test_with_inactive_conditional",
            file,
            13,
            false
        )
        .unwrap()
        .as_deref(),
        Some("direct_test_with_inactive_conditional")
    );
}

#[test]
fn test_conventions_per_language() {
    assert!(looks_like_test("go", "TestAdd", "pkg/add_test.go"));
    assert!(looks_like_test("go", "pkg.TestAdd", "pkg/add_test.go"));
    assert!(!looks_like_test("go", "helper", "pkg/add_test.go"));
    assert!(looks_like_test(
        "swift",
        "MathTests.testAdds()",
        "Tests/MathTests/MathTests.swift"
    ));
    assert!(!looks_like_test(
        "rust",
        "adds_numbers",
        "crates/a/tests/it.rs"
    ));
    assert!(looks_like_test("python", "test_adds", "tests/test_math.py"));
    assert!(!looks_like_test("typescript", "adds", "src/math.test.ts"));
    assert!(looks_like_test(
        "swift",
        "testAdds",
        "Tests/MathTests/MathTests.swift"
    ));
}

#[test]
fn test_commands_select_tests() {
    let tools = crate::verify::ProjectTools::default();
    let t = |name: &str| Symbol {
        name: name.into(),
        file: "x".into(),
        line: 1,
        col: 1,
    };
    // Go runs the packages that hold the tests, each once (#371).
    let in_file = |name: &str, file: &str| Symbol {
        name: name.into(),
        file: file.into(),
        line: 1,
        col: 1,
    };
    assert_eq!(
        test_command(
            "go",
            &tools,
            &[
                in_file("TestA", "internal/push/a_test.go"),
                in_file("pkg.TestB", "internal/push/b_test.go"),
                in_file("TestC", "cmd/tool/c_test.go"),
                in_file("TestD", "main_test.go"),
            ]
        )
        .unwrap()
        .join(" "),
        "go test . ./cmd/tool ./internal/push -run ^(TestA|TestB|TestC|TestD)$"
    );
    assert_eq!(
        test_command("rust", &tools, &[t("a"), t("b")])
            .unwrap()
            .join(" "),
        "sh -c for test_name in \"$@\"; do cargo test --workspace -- \"$test_name\" || exit; done prod-code-impact a b"
    );
    let py = test_command("python", &tools, &[t("test_a")]).unwrap();
    assert!(py.ends_with(&["-k".to_string(), "test_a".to_string()]));
    // ctest selects registered tests by name (#201).
    let cpp = test_command("cpp", &tools, &[t("Price.Doubles"), t("adds up")])
        .unwrap()
        .join(" ");
    assert!(
        cpp.contains("ctest") && cpp.contains("-R '^(Price.Doubles|adds up)$'"),
        "{cpp}"
    );
    let ts = test_command("typescript", &tools, &[t("tests/a.test.ts"), t("adds")]).unwrap();
    assert!(ts.ends_with(&[
        "tests/a.test.ts".to_string(),
        "-t".to_string(),
        "adds".to_string()
    ]));
    assert!(test_command("go", &tools, &[]).is_none());
}

#[test]
fn a_test_is_found_by_attribute_registration_or_test_case_class() {
    let rust =
        "mod checks {\n    #[tokio::test]\n    async fn prices() {}\n    fn helper() {}\n}\n";
    assert_eq!(
        test_marker("rust", rust, 3, "prices").as_deref(),
        Some("prices")
    );
    assert_eq!(test_marker("rust", rust, 4, "helper"), None);
    let multiline = "// #[test]\nconst TEXT: &str = \"#[test]\";\n#[tokio::test(\n    flavor = \"current_thread\"\n)]\nasync fn qualified() {}\n#[rstest]\nfn parameterized() {}\n#[test_case(1; 2)]\nfn cases() {}\nfn test_helper() {}\n";
    assert_eq!(
        test_marker("rust", multiline, 6, "checks::qualified").as_deref(),
        Some("checks::qualified")
    );
    assert!(test_marker("rust", multiline, 8, "parameterized").is_some());
    assert!(test_marker("rust", multiline, 10, "cases").is_some());
    assert_eq!(test_marker("rust", multiline, 11, "test_helper"), None);
    let swift = "@Test func adds() {}\nfunc plain() {}\n";
    assert!(test_marker("swift", swift, 1, "adds()").is_some());
    assert!(test_marker("swift", swift, 2, "plain()").is_none());
    let cpp = "#include <gtest/gtest.h>\nTEST(Price, Doubles) {\n  EXPECT_EQ(price(2, 3), 6);\n}\nTEST_CASE(\"adds up\") {\n}\n";
    assert_eq!(
        test_marker("cpp", cpp, 2, "TestBody").as_deref(),
        Some("Price.Doubles")
    );
    assert_eq!(
        test_marker("cpp", cpp, 3, "TestBody").as_deref(),
        Some("Price.Doubles")
    );
    assert_eq!(test_marker("cpp", cpp, 5, "x").as_deref(), Some("adds up"));
    assert_eq!(
        registration("TEST_F(Suite, Name)"),
        Some("Suite.Name".into())
    );
    assert_eq!(registration("TEST(, x)"), None);
    let py = "import unittest\n\nclass PriceChecks(unittest.TestCase):\n    def test_doubles(self):\n        pass\n\n    def helper(self):\n        pass\n\nclass Other:\n    def test_not(self):\n        pass\n";
    assert!(test_marker("python", py, 4, "test_doubles").is_some());
    assert!(test_marker("python", py, 7, "helper").is_none());
    assert!(test_marker("python", py, 11, "test_not").is_none());
    assert!(test_marker("go", "func TestX(t *testing.T) {}\n", 1, "TestX").is_none());
    assert!(test_marker("rust", "", 9, "x").is_none());
}

#[test]
fn the_whole_suite_runs_when_the_selection_cannot_be_trusted() {
    let mut report = ImpactReport {
        language: "rust".into(),
        base: "HEAD".into(),
        changed_files: vec!["src/lib.rs".into()],
        changed: vec![Symbol {
            name: "price".into(),
            file: "src/lib.rs".into(),
            line: 3,
            col: 8,
        }],
        callers: Vec::new(),
        tests: vec![Symbol {
            name: "prices".into(),
            file: "src/lib.rs".into(),
            line: 9,
            col: 8,
        }],
        test_command: None,
        unattributed_files: Vec::new(),
        index: None,
        reaches: Vec::new(),
        incomplete: Vec::new(),
        signature_warnings: Vec::new(),
    };
    assert_eq!(report.full_suite_reason(), None);
    // Tests reached, but no way to select them.
    let decision = report.ci_decision();
    assert_eq!(decision.run, CiRun::WholeSuite);
    assert!(
        decision.why.contains("cannot be selected"),
        "{}",
        decision.why
    );
    report.test_command = Some(vec!["cargo".into(), "test".into()]);
    assert_eq!(
        report.ci_decision(),
        CiDecision {
            run: CiRun::Selected(vec!["cargo".into(), "test".into()]),
            why: "1 test(s) that reach the change".into(),
        }
    );
    // A gap makes the selection untrustworthy, whatever it selected.
    report.incomplete = vec![Gap::Deleted {
        file: "src/gone.rs".into(),
    }];
    let decision = report.ci_decision();
    assert_eq!(decision.run, CiRun::WholeSuite);
    assert!(
        decision.why.contains("src/gone.rs was deleted"),
        "{}",
        decision.why
    );
    assert!(report.render().contains("incomplete analysis"));
    assert!(
        report
            .ci_summary(None, "x")
            .contains("- src/gone.rs was deleted")
    );
    report.incomplete.clear();
    report.test_command = None;
    let summary = report.ci_summary(Some(&["cargo".into(), "test".into()]), "the selection");
    assert!(
        summary.contains("| `price` | `src/lib.rs:3` |"),
        "{summary}"
    );
    assert!(summary.contains("- `prices` (`src/lib.rs:9`)"), "{summary}");
    assert!(
        summary.contains("Ran `cargo test`: the selection."),
        "{summary}"
    );
    assert!(
        report
            .ci_summary(None, "no test reaches them")
            .contains("Ran nothing")
    );
    report.unattributed_files = vec!["Cargo.toml".into()];
    assert!(report.full_suite_reason().unwrap().contains("Cargo.toml"));
    report.index = Some(IndexBuild {
        command: "swift build".into(),
        ok: false,
        duration_ms: 1,
    });
    assert!(report.full_suite_reason().unwrap().contains("index"));
}
