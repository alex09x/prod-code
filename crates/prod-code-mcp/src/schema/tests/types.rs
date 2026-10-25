/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::BTreeMap;
use std::path::PathBuf;

use super::super::types::{AcrossRepos, SchemaRename, lsp_end_position};

fn plan(root: &std::path::Path, file: &str, old: &str, new: &str) -> SchemaRename {
    let path = root.join(file);
    std::fs::write(&path, old).unwrap();
    SchemaRename {
        field: "order_id".to_string(),
        to: "trade_id".to_string(),
        root: root.to_path_buf(),
        rewritten: vec![(path.clone(), new.to_string())],
        original_lines: BTreeMap::from([(path.clone(), old.lines().count())]),
        original_ends: BTreeMap::from([(path, lsp_end_position(old))]),
        summary: vec!["text: 1 occurrence rewritten".to_string()],
        left: Vec::new(),
        diagnostics: Vec::new(),
        applied: false,
    }
}

#[test]
fn lsp_end_position_counts_utf16_units_and_ignores_carriage_returns() {
    let cases = [
        ("", (0, 0)),
        ("abc", (0, 3)),
        ("a\nbc\n", (2, 0)),
        ("a\r\nbc", (1, 2)),
        ("😀x", (0, 3)),
        ("é\n😀", (1, 2)),
    ];
    for (text, want) in cases {
        assert_eq!(lsp_end_position(text), want, "{text:?}");
    }
}

#[test]
fn a_rename_report_shows_the_diff_and_what_was_left() {
    let dir = tempfile::Builder::new().prefix("sc").tempdir().unwrap();
    let mut rename = plan(
        dir.path(),
        "a.txt",
        "one\norder_id\nthree\n",
        "one\ntrade_id\nthree\n",
    );
    rename.left = vec!["README.md:3 order_id".to_string()];
    let out = rename.render(10_000);
    assert!(out.starts_with(
        "`order_id` → `trade_id`\n\n- text: 1 occurrence rewritten\n\n2 changed line(s) in 1 file(s)\n\n"
    ));
    assert!(out.contains(
        "\n\n--- a/a.txt\n+++ b/a.txt\n@@ -1,3 +1,3 @@\n one\n-order_id\n+trade_id\n three\n"
    ));
    assert!(out.contains("not rewritten (1), because no analyzer owns them"));
    assert!(out.contains("  README.md:3 order_id\n"));
    assert!(out.contains("the analyzers accept the result: 0 errors"));
    assert!(out.ends_with("nothing was written; pass `apply: true` to make these edits\n"));

    rename.applied = true;
    rename.diagnostics = vec!["a.txt:2: bad".to_string()];
    let out = rename.render(10_000);
    assert!(out.contains("the analyzers reject the result:\n  a.txt:2: bad\n"));
    assert!(out.ends_with("[applied to 1 file(s)]\n"));
}

#[test]
fn a_long_diff_is_cut_to_the_budget() {
    let dir = tempfile::Builder::new().prefix("sc").tempdir().unwrap();
    let rename = plan(
        dir.path(),
        "a.txt",
        "one\norder_id\nthree\n",
        "one\ntrade_id\nthree\n",
    );
    let out = rename.render(12);
    assert!(out.contains("file(s)\n\n--- a/a.txt\n\n… diff truncated\n"));
    assert!(!out.contains("+trade_id"));
}

#[test]
fn a_workspace_edit_replaces_each_file_up_to_its_old_end() {
    let dir = tempfile::Builder::new().prefix("sc").tempdir().unwrap();
    let rename = plan(dir.path(), "a.txt", "one\norder_id\n", "one\ntrade_id\n");
    let edit = rename.workspace_edit();
    let change = &edit["documentChanges"][0];
    assert_eq!(change["edits"][0]["range"]["end"]["line"], 2);
    assert_eq!(change["edits"][0]["range"]["end"]["character"], 0);
    assert_eq!(change["edits"][0]["newText"], "one\ntrade_id\n");
}

#[test]
fn across_repos_reports_each_repo_and_the_ones_without_the_field() {
    let a = tempfile::Builder::new().prefix("sc").tempdir().unwrap();
    let b = tempfile::Builder::new().prefix("sc").tempdir().unwrap();
    let mut across = AcrossRepos {
        repos: vec![
            plan(a.path(), "a.txt", "order_id\n", "trade_id\n"),
            plan(b.path(), "b.txt", "order_id\n", "trade_id\n"),
        ],
        missing: vec![PathBuf::from("/nowhere/repo")],
        applied: false,
    };
    assert!(across.clean());
    let edit = across.workspace_edit();
    assert_eq!(edit["documentChanges"].as_array().unwrap().len(), 2);
    let out = across.render(10_000);
    assert!(out.contains(&format!(
        "## {}\n\n`order_id` → `trade_id`",
        a.path().display()
    )));
    assert!(out.contains(&format!(
        "## {}\n\n`order_id` → `trade_id`",
        b.path().display()
    )));
    assert!(out.contains("## /nowhere/repo\n\nthe field does not appear here\n\n"));
    assert!(out.ends_with(
        "nothing was written in any of the 3 repositories; `apply: true` writes them all or none\n"
    ));

    across.applied = true;
    across.repos[1].diagnostics = vec!["boom".to_string()];
    assert!(!across.clean());
    assert!(
        across
            .render(10_000)
            .ends_with("[applied to 2 repositories together]\n")
    );

    let empty = AcrossRepos {
        repos: Vec::new(),
        missing: Vec::new(),
        applied: false,
    };
    assert!(empty.clean());
    assert_eq!(
        empty.render(10),
        "nothing was written in any of the 0 repositories; `apply: true` writes them all or none\n"
    );
}
