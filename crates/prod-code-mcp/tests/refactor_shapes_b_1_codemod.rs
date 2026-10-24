/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The structural codemod and search over a checkout: which files are visited, what the preview
//! and the applied rewrite say, how a bad rule or scope is refused, and how results are rendered.

use prod_code_mcp::codemod::{
    CodemodOutcome, CodemodRule, CompiledPattern, StructuralMatchItem, StructuralSearchResult,
    byte_to_line_col, run_codemod, run_structural_search,
};
use prod_code_testkit::Workspace;
use std::collections::BTreeMap;
use std::path::Path;

const UNWRAP: &str = "$a.unwrap() ==>> $a.expect(\"set\")";
const A_RS: &str = "fn a(x: Option<u8>) -> u8 {\n    x.unwrap()\n}\n";
const A_RS_AFTER: &str = "fn a(x: Option<u8>) -> u8 {\n    x.expect(\"set\")\n}\n";

fn checkout() -> Workspace {
    Workspace::new(&[
        ("src/a.rs", A_RS),
        ("src/b.rs", "fn b() {}\n"),
        ("node_modules/n.rs", A_RS),
        (".hidden/h.rs", A_RS),
        ("notes.txt", "x.unwrap()\n"),
        (
            "pkgdir/lib.go",
            "package p\n\nfunc f() {\n\terrors.Wrap(err, \"m\")\n}\n",
        ),
    ])
}

#[test]
fn a_preview_reports_the_diff_and_writes_nothing_while_apply_rewrites_the_file() {
    let ws = checkout();
    let root = ws.root();
    let scope = root.join("src");
    let preview = run_codemod(&root, UNWRAP, Some(&scope), false).unwrap();
    assert_eq!(preview.rule, UNWRAP);
    assert_eq!(
        (
            preview.files_scanned,
            preview.files_matched,
            preview.total_matches,
            preview.changed_lines
        ),
        (2, 1, 1, 2)
    );
    assert_eq!(
        preview.diff,
        "--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,3 +1,3 @@\n fn a(x: Option<u8>) -> u8 {\n-    x.unwrap()\n+    x.expect(\"set\")\n }\n"
    );
    assert_eq!(
        preview.rewritten_files,
        vec![(root.join("src/a.rs"), A_RS_AFTER.to_string())]
    );
    assert_eq!(ws.read("src/a.rs"), A_RS);

    // Without a scope the whole checkout is walked, minus vendored, hidden and non-code files.
    let whole = run_codemod(&root, UNWRAP, None, true).unwrap();
    assert_eq!((whole.files_scanned, whole.files_matched), (3, 1));
    assert_eq!(ws.read("src/a.rs"), A_RS_AFTER);
    assert_eq!(ws.read("node_modules/n.rs"), A_RS);
    assert_eq!(ws.read(".hidden/h.rs"), A_RS);

    let again = run_codemod(&root, UNWRAP, Some(&root.join("src/b.rs")), false).unwrap();
    assert_eq!((again.files_scanned, again.files_matched), (1, 0));
    assert_eq!(again.diff, "");
    assert!(again.rewritten_files.is_empty());
}

#[cfg(unix)]
#[test]
fn a_codemod_scope_outside_the_checkout_or_through_a_link_is_refused() {
    let ws = checkout();
    let root = ws.root();
    std::os::unix::fs::symlink(root.join("src"), root.join("linked")).unwrap();
    let message = |scope: &Path| {
        run_codemod(&root, UNWRAP, Some(scope), false)
            .unwrap_err()
            .to_string()
    };
    assert_eq!(
        message(Path::new("/definitely/elsewhere")),
        "codemod scope is outside the workspace"
    );
    assert_eq!(
        message(&root.join("linked")),
        "codemod scope traverses a symlink"
    );
    assert!(message(&root.join("src/none.rs")).contains("No such file"));
    // A link inside a walked directory is skipped rather than followed.
    std::os::unix::fs::symlink(root.join("src/a.rs"), root.join("src/link.rs")).unwrap();
    let walked = run_codemod(&root, UNWRAP, Some(&root.join("src")), false).unwrap();
    assert_eq!(walked.files_scanned, 2);
}

#[test]
fn rules_and_patterns_that_are_malformed_are_named() {
    let ws = checkout();
    let refuse = |rule: &str| CodemodRule::parse(rule).unwrap_err().to_string();
    assert_eq!(
        refuse("x.unwrap()"),
        "a rule must be `pattern ==>> replacement`, got: x.unwrap()"
    );
    assert_eq!(
        refuse("a ==>> b ==>> c"),
        "a rule must be `pattern ==>> replacement`, got: a ==>> b ==>> c"
    );
    assert_eq!(refuse("  ==>> b"), "pattern in rule cannot be empty");
    assert_eq!(
        run_codemod(&ws.root(), "no arrow", None, false)
            .unwrap_err()
            .to_string(),
        "a rule must be `pattern ==>> replacement`, got: no arrow"
    );
    assert_eq!(
        CompiledPattern::parse("  ").unwrap_err().to_string(),
        "pattern cannot be empty"
    );
    assert_eq!(
        run_structural_search(&ws.root(), "", None)
            .unwrap_err()
            .to_string(),
        "pattern cannot be empty"
    );
    // Only identifiers of two characters or more that are not metavariables must be present.
    let pattern = CompiledPattern::parse("foo.bar($x, ab, a, foo)").unwrap();
    assert_eq!(pattern.required_literals, vec!["ab", "bar", "foo"]);
    assert_eq!(pattern.raw, "foo.bar($x, ab, a, foo)");
}

#[test]
fn structural_search_accepts_a_directory_a_file_or_a_workspace_relative_scope() {
    let ws = checkout();
    let root = ws.root();
    let pattern = "errors.Wrap($err, $msg)";
    let all = run_structural_search(&root, pattern, None).unwrap();
    assert_eq!(
        (all.files_scanned, all.files_matched, all.total_matches),
        (3, 1, 1)
    );
    let hit = &all.matches[0];
    assert_eq!(
        (hit.file.as_str(), hit.line, hit.col),
        ("pkgdir/lib.go", 4, 2)
    );
    assert_eq!(hit.matched_text, "errors.Wrap(err, \"m\")");
    let bindings: Vec<(&str, &str)> = hit
        .bindings
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    assert_eq!(bindings, vec![("err", "err"), ("msg", "\"m\"")]);

    for scope in [
        root.join("pkgdir"),
        root.join("pkgdir/lib.go"),
        Path::new("pkgdir").to_path_buf(),
        Path::new("pkgdir/lib.go").to_path_buf(),
    ] {
        let found = run_structural_search(&root, pattern, Some(&scope)).unwrap();
        assert_eq!(
            (found.files_scanned, found.total_matches),
            (1, 1),
            "{scope:?}"
        );
    }
    let missing = run_structural_search(&root, pattern, Some(Path::new("nowhere"))).unwrap();
    assert_eq!((missing.files_scanned, missing.total_matches), (0, 0));
}

#[test]
fn outcomes_and_search_results_render_with_their_limits() {
    let outcome = |diff: &str, matched: usize| CodemodOutcome {
        rule: "r".to_string(),
        files_scanned: 2,
        files_matched: matched,
        total_matches: matched,
        changed_lines: 3,
        diff: diff.to_string(),
        rewritten_files: Vec::new(),
        elapsed_ms: 0.5,
    };
    let head = "`r`\n3 changed line(s) in 1 file(s) (2 scanned in 0.50ms)\n\n";
    assert_eq!(
        outcome("", 1).render(10),
        format!("{head}matches nothing\n")
    );
    assert_eq!(outcome("+a\n", 1).render(10), format!("{head}+a\n"));
    // The limit counts characters, not bytes.
    assert_eq!(
        outcome("ééééé", 1).render(3),
        format!("{head}ééé\n… diff truncated\n")
    );

    let item = |file: &str, bindings: &[(&str, &str)]| StructuralMatchItem {
        file: file.to_string(),
        line: 2,
        col: 5,
        matched_text: "x.unwrap()\nmore".to_string(),
        bindings: bindings
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<BTreeMap<_, _>>(),
    };
    let search = StructuralSearchResult {
        pattern: "$a.unwrap()".to_string(),
        files_scanned: 3,
        files_matched: 1,
        total_matches: 2,
        matches: vec![item("src/a.rs", &[("a", "x")]), item("src/b.rs", &[])],
        elapsed_ms: 1.5,
    };
    let bar = "─".repeat(52);
    let top = format!(
        "⚡ prod-code Structural AST Search: `$a.unwrap()`\n{bar}\n2 match(es) in 1 file(s) (3 scanned in 1.50ms)\n\n"
    );
    assert_eq!(
        search.render(1),
        format!(
            "{top}  • src/a.rs:2:5  x.unwrap()\n    └─ [$a = x]\n\n  … and 1 more match(es) truncated\n"
        )
    );
    assert_eq!(
        search.render(5),
        format!(
            "{top}  • src/a.rs:2:5  x.unwrap()\n    └─ [$a = x]\n  • src/b.rs:2:5  x.unwrap()\n"
        )
    );
    let none = StructuralSearchResult {
        matches: Vec::new(),
        total_matches: 0,
        files_matched: 0,
        files_scanned: 0,
        elapsed_ms: 0.0,
        ..search
    };
    assert_eq!(
        none.render(5),
        format!(
            "⚡ prod-code Structural AST Search: `$a.unwrap()`\n{bar}\n0 match(es) in 0 file(s) (0 scanned in 0.00ms)\n\n✓ No matches found for pattern.\n"
        )
    );
}

#[test]
fn byte_offsets_become_one_based_lines_and_columns() {
    for (source, offset, expected) in [
        ("", 0, (1, 1)),
        ("ab\ncd", 0, (1, 1)),
        ("ab\ncd", 1, (1, 2)),
        ("ab\ncd", 3, (2, 1)),
        ("ab\ncd", 4, (2, 2)),
        ("ab\ncd", 99, (2, 3)),
        // A multi-byte character is one column.
        ("é\nx", 2, (1, 2)),
        ("é\nx", 3, (2, 1)),
    ] {
        assert_eq!(
            byte_to_line_col(source, offset),
            expected,
            "{source:?}@{offset}"
        );
    }
}
