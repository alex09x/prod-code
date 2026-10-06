/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::diff::{apply_hypothesis, unified_diff};
use super::rank::{rank, test_counts};
use super::report::render_report;
use super::spec::{parse_specs, relative_edit_path};
use super::types::{HypothesisEdit, HypothesisOutcome, HypothesisSpec, ShadowOutcome};
use std::path::Path;

fn outcome(
    name: &str,
    exit: Option<i32>,
    tests: Option<(u64, u64)>,
    lines: usize,
) -> HypothesisOutcome {
    HypothesisOutcome {
        name: name.to_string(),
        exit_code: exit,
        duration_ms: 1000,
        timed_out: false,
        error: None,
        output: String::new(),
        output_len: 0,
        tests,
        diff: String::new(),
        changed_lines: lines,
    }
}

#[test]
fn ranking_prefers_passing_then_fewer_failures_then_smaller_diff() {
    let results = vec![
        outcome("big", Some(0), Some((10, 0)), 40),
        outcome("broken", Some(101), Some((9, 1)), 5),
        outcome("small", Some(0), Some((10, 0)), 8),
        outcome("worse", Some(101), Some((7, 3)), 2),
    ];
    assert_eq!(rank(&results), vec![2, 0, 1, 3]);
    let none_passed = vec![
        outcome("a", Some(1), Some((3, 2)), 1),
        outcome("b", Some(1), Some((4, 1)), 9),
    ];
    assert_eq!(rank(&none_passed), vec![1, 0]);
}

#[test]
fn test_counts_follow_the_command() {
    let cargo = ["cargo", "test", "-p", "x"].map(String::from);
    assert_eq!(
        test_counts(&cargo, "test result: ok. 6 passed; 0 failed; 0 ignored\n"),
        Some((6, 0))
    );
    let pytest = ["pytest", "-q"].map(String::from);
    assert_eq!(
        test_counts(&pytest, "===== 2 failed, 5 passed in 0.10s =====\n"),
        Some((5, 2))
    );
    let build = ["cargo", "build"].map(String::from);
    assert_eq!(test_counts(&build, "Finished"), None);
}

#[test]
fn unified_diff_marks_new_changed_and_deleted_files() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.rs"), "fn a() {}\nfn b() {}\n").unwrap();
    std::fs::write(root.path().join("gone.rs"), "x\n").unwrap();
    let spec = HypothesisSpec {
        name: "h".to_string(),
        edits: vec![
            HypothesisEdit {
                relative_path: "a.rs".to_string(),
                text: Some("fn a() {}\nfn c() {}\n".to_string()),
            },
            HypothesisEdit {
                relative_path: "new.rs".to_string(),
                text: Some("fn n() {}\n".to_string()),
            },
            HypothesisEdit {
                relative_path: "gone.rs".to_string(),
                text: None,
            },
            HypothesisEdit {
                relative_path: "same.rs".to_string(),
                text: None,
            },
        ],
    };
    let (diff, changed) = unified_diff(root.path(), &spec);
    assert!(diff.contains("--- a/a.rs\n+++ b/a.rs\n"), "{diff}");
    assert!(diff.contains("-fn b() {}\n+fn c() {}\n"), "{diff}");
    assert!(diff.contains("--- /dev/null\n+++ b/new.rs\n"), "{diff}");
    assert!(diff.contains("--- a/gone.rs\n+++ /dev/null\n"), "{diff}");
    assert!(!diff.contains("same.rs"));
    assert_eq!(changed, 2 + 1 + 1);
    let touched = apply_hypothesis(root.path(), &spec).unwrap();
    assert_eq!(touched.len(), 4);
    assert!(root.path().join("new.rs").exists() && !root.path().join("gone.rs").exists());
}

#[test]
fn relative_edit_path_resolves_and_rejects_paths_outside_the_workspace() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("src")).unwrap();

    assert_eq!(
        relative_edit_path(root.path(), Path::new("src/a.rs")).unwrap(),
        "src/a.rs"
    );
    let abs = root.path().join("src/b.rs");
    assert_eq!(relative_edit_path(root.path(), &abs).unwrap(), "src/b.rs");

    let err = relative_edit_path(root.path(), Path::new("/etc/passwd")).unwrap_err();
    assert!(format!("{err:#}").contains("outside the workspace"));

    let err = relative_edit_path(root.path(), root.path()).unwrap_err();
    assert!(format!("{err:#}").contains("needs a file path"));
}

#[test]
fn parse_specs_builds_edits_from_inline_text_files_and_deletes_with_defaults() {
    let root = tempfile::tempdir().unwrap();
    let content_dir = tempfile::tempdir().unwrap();
    std::fs::write(content_dir.path().join("body.rs"), "fn a() {}\n").unwrap();

    let json = serde_json::json!({
        "hypotheses": [
            {
                "edits": [
                    { "path": "src/a.rs", "new_text": "fn a() {}\n" },
                    { "path": "src/b.rs", "file": "body.rs" }
                ]
            },
            {
                "name": "  named  ",
                "edits": [ { "path": "src/c.rs", "new_text": "x\n" } ],
                "delete": [ "src/old.rs" ]
            }
        ]
    });
    let specs = parse_specs(root.path(), &json, Some(content_dir.path())).unwrap();
    assert_eq!(specs.len(), 2);
    assert_eq!(
        specs[0].name, "h1",
        "an unnamed hypothesis gets a default name"
    );
    assert_eq!(specs[0].edits[0].relative_path, "src/a.rs");
    assert_eq!(specs[0].edits[0].text.as_deref(), Some("fn a() {}\n"));
    assert_eq!(specs[0].edits[1].relative_path, "src/b.rs");
    assert_eq!(
        specs[0].edits[1].text.as_deref(),
        Some("fn a() {}\n"),
        "the file's content is read relative to file_base"
    );
    assert_eq!(specs[1].name, "named", "the given name is trimmed");
    assert_eq!(specs[1].edits.len(), 2);
    assert_eq!(specs[1].edits[1].relative_path, "src/old.rs");
    assert_eq!(specs[1].edits[1].text, None, "a delete has no text");

    assert!(parse_specs(root.path(), &serde_json::json!({}), None).is_err());
    assert!(parse_specs(root.path(), &serde_json::json!({"hypotheses": []}), None).is_err());
    let no_path = serde_json::json!({"hypotheses":[{"edits":[{"new_text":"x"}]}]});
    assert!(parse_specs(root.path(), &no_path, None).is_err());
    let no_text = serde_json::json!({"hypotheses":[{"edits":[{"path":"a.rs"}]}]});
    let err = parse_specs(root.path(), &no_text, None).unwrap_err();
    assert!(format!("{err:#}").contains("needs 'new_text' or 'file'"));
}

#[test]
fn render_report_shows_applied_files_and_the_closest_hypothesis_when_none_passed() {
    let mut results = vec![outcome("ok", Some(0), Some((1, 0)), 0)];
    results[0].diff = "--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n".to_string();
    let ranking = rank(&results);
    let winner = Some(ranking[0]);
    let shadow = ShadowOutcome {
        mode: "in-place".to_string(),
        server_workspace_root: "/srv/ws".to_string(),
        results,
        ranking,
        winner,
        in_memory: false,
    };
    let text = render_report(
        &shadow,
        &["cargo".to_string(), "test".to_string()],
        Some(&["src/a.rs".to_string()]),
        100,
    );
    assert!(
        text.contains("[applied 1 file(s) to the checkout: src/a.rs]"),
        "{text}"
    );

    let results2 = vec![
        outcome("a", Some(1), None, 3),
        outcome("b", Some(1), None, 1),
    ];
    let ranking2 = rank(&results2);
    let shadow2 = ShadowOutcome {
        mode: "overlay".to_string(),
        server_workspace_root: "/srv".to_string(),
        results: results2,
        ranking: ranking2,
        winner: None,
        in_memory: false,
    };
    let text2 = render_report(&shadow2, &["go".to_string(), "test".to_string()], None, 100);
    assert!(text2.contains("no hypothesis passed; closest:"), "{text2}");
}

#[test]
fn report_names_the_winner_and_shows_failing_output() {
    let mut results = vec![
        outcome("ok", Some(0), Some((3, 0)), 2),
        outcome("bad", Some(101), Some((2, 1)), 2),
    ];
    results[0].diff = "--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n".to_string();
    results[1].output = "thread 'main' panicked\n".to_string();
    results[1].output_len = 24;
    let ranking = rank(&results);
    let winner = Some(ranking[0]);
    let shadow = ShadowOutcome {
        mode: "overlay".to_string(),
        server_workspace_root: "/srv/ws".to_string(),
        results,
        ranking,
        winner,
        in_memory: false,
    };
    let text = render_report(
        &shadow,
        &["cargo".to_string(), "test".to_string()],
        None,
        100,
    );
    assert!(text.contains("<- winner"), "{text}");
    assert!(text.contains("winner: ok\n--- a/x"), "{text}");
    assert!(text.contains("--- bad output"), "{text}");
    assert!(text.contains("panicked"), "{text}");
}

#[test]
fn render_report_retains_compiler_errors_when_warnings_exceed_tail_cap() {
    let mut results = vec![
        outcome("ok", Some(0), Some((1, 0)), 1),
        outcome("compile_fail", Some(101), None, 1),
    ];
    results[0].diff = "--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n".to_string();

    let mut output = String::new();
    output.push_str("   Compiling server v0.1.0 (/ws/server)\n");
    output.push_str("error[E0061]: this function takes 2 arguments but 1 argument was supplied\n");
    output.push_str("  --> server/src/lib.rs:42:15\n");
    output.push_str("   |\n");
    output.push_str("42 |     calculate(foo);\n");
    output.push_str("   |     ^^^^^^^^^ --- supplied 1 argument\n");
    output.push_str("   |     |\n");
    output.push_str("   |     expected 2 arguments\n");
    output.push_str("   |\n");
    output.push_str("help: provide the argument: `, bar`\n\n");
    // Append 5 KB of warnings, exceeding the 1000 char tail cap
    for i in 0..60 {
        output.push_str(&format!(
            "warning: call to unsafe function `{i}` is unsafe and requires unsafe block (error E0133)\n  --> server/src/lib.rs:{i}:5\n"
        ));
    }
    output.push_str("error: could not compile `server` (lib test) due to 1 previous error; 60 warnings emitted\n");

    results[1].output = output.clone();
    results[1].output_len = output.len() as u64;

    let ranking = rank(&results);
    let shadow = ShadowOutcome {
        mode: "overlay".to_string(),
        server_workspace_root: "/srv/ws".to_string(),
        results,
        ranking,
        winner: Some(0),
        in_memory: false,
    };

    let text = render_report(
        &shadow,
        &[
            "cargo".to_string(),
            "test".to_string(),
            "-p".to_string(),
            "server".to_string(),
        ],
        None,
        1000,
    );

    assert!(text.contains("error[E0061]"), "{text}");
    assert!(text.contains("server/src/lib.rs:42:15"), "{text}");
    assert!(text.contains("calculate(foo)"), "{text}");
    assert!(
        text.contains("help: provide the argument: `, bar`"),
        "{text}"
    );
    assert!(text.contains("could not compile"), "{text}");
}
