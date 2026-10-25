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
use prod_code_mcp::diagnostics::{parse_items, set_aside_derive_expansions, set_aside_preexisting};

#[tokio::test]
async fn baseline_authority_cannot_transfer_through_insertions_or_header_delimiters() {
    let mut failures = Vec::new();
    for (path, before, after, code, message) in [
        (
            "src/main.ts",
            "function first() {\n    missing();\n}\n",
            "function first() {\n}\nfunction second() {\n    missing();\n}\n",
            "2304",
            "Cannot find name 'missing'",
        ),
        (
            "src/lib.rs",
            "fn first(_x: [u8; 1])\n{\n    missing();\n}\n",
            "fn second(_x: [u8; 1])\n{\n    missing();\n}\n",
            "E0425",
            "cannot find function `missing`",
        ),
        (
            "src/main.ts",
            "function first() {\n}\nfunction second() {\n    missing();\n}\n",
            "function first() {\n    missing();\n}\n",
            "2304",
            "Cannot find name 'missing'",
        ),
        (
            "src/lib.rs",
            "fn first(_x: [u8; { 1; 1 }])\n{\n    missing();\n}\n",
            "fn second(_x: [u8; { 1; 1 }])\n{\n    missing();\n}\n",
            "E0425",
            "cannot find function `missing`",
        ),
        (
            "src/main.py",
            "def first():\n    missing()\n",
            "def first():\n    pass\ndef second():\n    missing()\n",
            "reportUndefinedVariable",
            "\"missing\" is not defined",
        ),
    ] {
        let ws = Workspace::new(&[(path, before)]);
        let diagnostic = move |text: &str| {
            let line = text
                .lines()
                .position(|line| line.contains("missing"))
                .unwrap() as u32
                + 1;
            let mut answer = answers::error_at(line, 5, code, message);
            answer["items"][0]["range"]["end"] = json!({"line": line - 1, "character": 11});
            answer
        };
        let mut filtered = parse_items(path, &diagnostic(after));
        set_aside_preexisting(
            &mut filtered,
            after,
            &parse_items(path, &diagnostic(before)),
            before,
        );
        // Exercise all gates even on the broken implementation, then report their states.
        let gw = gateway_for(&ws, path, diagnostic).await;
        let report = validate_text(gw.addr(), &ws.root(), &ws.path(path), after)
            .await
            .unwrap();
        let batch = validate_texts(gw.addr(), &ws.root(), &[(ws.path(path), after.into())], &[])
            .await
            .unwrap();
        let public = execute_tool(
            gw.addr(),
            &ws.root(),
            "code_validate_edits",
            json!({
                "edits": [{"path": path, "new_text": after}]
            }),
        )
        .await
        .unwrap();
        if filtered.ok()
            || report.ok()
            || batch[0].ok()
            || !public.is_error
            || report.errors != 1
            || !report.preexisting.is_empty()
        {
            failures.push(format!(
                "{path} ({before:?}): filter={}, report={}, batch={}, public_is_error={}",
                filtered.ok(),
                report.ok(),
                batch[0].ok(),
                public.is_error
            ));
        }
        assert_eq!(ws.read(path), before);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn baseline_context_survives_harmless_comments_and_array_parameter_shifts() {
    for (path, before, code, comment) in [
        (
            "src/main.ts",
            "function first() {\n    missing();\n}\n",
            "2304",
            "//",
        ),
        (
            "src/lib.rs",
            "fn first(_x: [u8; 1])\n{\n    missing();\n}\n",
            "E0425",
            "//",
        ),
        (
            "src/lib.rs",
            "fn first(_x: [u8; { 1; 1 }])\n{\n    missing();\n}\n",
            "E0425",
            "//",
        ),
        (
            "src/main.py",
            "def first():\n    missing()\n",
            "reportUndefinedVariable",
            "#",
        ),
        (
            "src/main.pyi",
            "def first():\n    missing()\n",
            "reportUndefinedVariable",
            "#",
        ),
    ] {
        let ws = Workspace::new(&[(path, before)]);
        let gw = gateway_for(&ws, path, move |text| {
            let line = text
                .lines()
                .position(|line| line.contains("missing"))
                .unwrap() as u32
                + 1;
            answers::error_at(line, 5, code, "missing")
        })
        .await;
        for after in [
            format!("\n{before}"),
            format!("{comment} harmless insertion\n{before}"),
            before.replace(
                "    missing",
                &format!("    {comment} harmless insertion\n    missing"),
            ),
        ] {
            let report = validate_text(gw.addr(), &ws.root(), &ws.path(path), &after)
                .await
                .unwrap();
            assert!(report.ok(), "{path}: {report:?}");
            assert_eq!(report.preexisting.len(), 1);
            let public = execute_tool(
                gw.addr(),
                &ws.root(),
                "code_validate_edits",
                json!({
                    "edits": [{"path": path, "new_text": after}]
                }),
            )
            .await
            .unwrap();
            assert!(!public.is_error, "{path}: {public:?}");
            assert_eq!(ws.read(path), before);
        }
    }
}

#[test]
fn other_languages_require_snapshot_proof_when_structural_context_is_unavailable() {
    let before_text = "function first() {\n    missing();\n}\nfunction second() {}\n";
    let after_text = "function first() {}\nfunction second() {\n    missing();\n}\n";
    let before = parse_items(
        "src/main.ts",
        &answers::error_at(2, 5, "2304", "Cannot find name 'missing'"),
    );
    let mut report = parse_items(
        "src/main.ts",
        &answers::error_at(3, 5, "2304", "Cannot find name 'missing'"),
    );
    set_aside_preexisting(&mut report, after_text, &before, before_text);
    assert!(!report.ok(), "unsupported scope proof must fail closed");
    let mut shifted = parse_items(
        "src/main.ts",
        &answers::error_at(3, 5, "2304", "Cannot find name 'missing'"),
    );
    set_aside_preexisting(
        &mut shifted,
        &format!("// insertion\n{before_text}"),
        &before,
        before_text,
    );
    assert!(
        shifted.ok(),
        "whole-snapshot shifts still have correspondence proof"
    );
}

#[tokio::test]
async fn relocating_a_line_to_another_scope_needs_more_than_a_unique_text_match() {
    for (before, after) in [
        (
            "fn first() {\n    missing();\n}\nfn second() {}\n",
            "fn first() {}\nfn second() {\n    missing();\n}\n",
        ),
        (
            "fn first()\n{\n    missing();\n}\n",
            "fn second()\n{\n    missing();\n}\n",
        ),
        (
            "impl First {\nfn run() {\n    missing();\n}\n}\n",
            "impl Second {\nfn run() {\n    missing();\n}\n}\n",
        ),
    ] {
        let ws = workspace(before);
        let gw = gateway(&ws, |text| {
            let line = text
                .lines()
                .position(|line| line.contains("missing"))
                .unwrap() as u32
                + 1;
            answers::error_at(line, 5, "E0425", "cannot find function `missing`")
        })
        .await;
        let report = validate_text(gw.addr(), &ws.root(), &ws.path("src/lib.rs"), after)
            .await
            .unwrap();
        assert!(
            !report.ok(),
            "a unique moved line is not occurrence proof: {report:?}"
        );
        assert!(report.preexisting.is_empty());
    }
}

#[tokio::test]
async fn stable_scope_context_preserves_baselines_across_other_mixed_edits() {
    let before =
        "fn run() {\n    let _s = r#\"} {\"#;\n    let _unrelated = 0;\n    missing();\n}\n";
    let after = "// a harmless }\nfn run() {\n    let _s = r#\"} {\"#;\n    let _unrelated = 1;\n    missing();\n}\n";
    let ws = workspace(before);
    let gw = gateway(&ws, |text| {
        let line = text
            .lines()
            .position(|line| line.contains("missing"))
            .unwrap() as u32
            + 1;
        answers::error_at(line, 5, "E0425", "cannot find function `missing`")
    })
    .await;
    let report = validate_text(gw.addr(), &ws.root(), &ws.path("src/lib.rs"), after)
        .await
        .unwrap();
    assert!(report.ok(), "stable context was lost: {report:?}");
    assert_eq!(report.preexisting.len(), 1);
}

#[test]
fn baseline_requires_the_same_column_source_and_complete_unchanged_range() {
    let text = "first(); second();\nunchanged\nlast();\n";
    let answer = answers::error_at(1, 1, "E0308", "same error");
    let before = parse_items("src/lib.rs", &answer);
    for change in ["column", "source", "range"] {
        let mut report = before.clone();
        match change {
            "column" => report.items[0].col = 10,
            "source" => report.items[0].source = Some("different analyzer".into()),
            "range" => report.items[0].end = Some((3, 2)),
            _ => unreachable!(),
        }
        set_aside_preexisting(&mut report, text, &before, text);
        assert!(!report.ok(), "{change}: {report:?}");
    }

    let mut before = before;
    before.items[0].end = Some((3, 2));
    let mut report = before.clone();
    set_aside_preexisting(
        &mut report,
        "first(); second();\nchanged\nlast();\n",
        &before,
        text,
    );
    assert!(
        !report.ok(),
        "changed interior is a changed diagnostic range"
    );
}

#[test]
fn duplicate_counts_and_moved_occurrences_cannot_consume_the_wrong_error() {
    let text = "fn a() {\nmissing();\n}\nfn b() {\nmissing();\n}\n";
    let before = parse_items("src/lib.rs", &answers::error_at(2, 1, "E0425", "missing"));
    let mut report = parse_items(
        "src/lib.rs",
        &json!({"kind": "full", "items": [
            answers::error_at(5, 1, "E0425", "missing")["items"][0],
            answers::error_at(2, 1, "E0425", "missing")["items"][0],
            answers::error_at(2, 1, "E0425", "missing")["items"][0]
        ]}),
    );
    set_aside_preexisting(&mut report, text, &before, text);
    assert_eq!(report.errors, 2);
    assert_eq!(report.preexisting.len(), 1);
    assert_eq!(report.preexisting[0].line, 2);
    assert_eq!(report.items[0].line, 5);
}

#[test]
fn large_insertion_still_maps_but_large_mixed_edits_fail_closed() {
    let text = (0..1500).map(|i| format!("line{i}\n")).collect::<String>();
    let before = parse_items("src/lib.rs", &answers::error_at(500, 1, "E0425", "missing"));
    let mut report = parse_items("src/lib.rs", &answers::error_at(501, 1, "E0425", "missing"));
    set_aside_preexisting(&mut report, &format!("// shift\n{text}"), &before, &text);
    assert!(
        report.ok(),
        "large pure insertion has linear-time correspondence proof"
    );

    let mut report = before.clone();
    let mixed = text.replacen("line1499", "changed", 1);
    set_aside_preexisting(&mut report, &mixed, &before, &text);
    assert!(!report.ok(), "comparison budget is not evidence of clean");
}

#[test]
fn retained_limitation_notes_preserve_existing_context_and_are_idempotent() {
    for (text, code, message) in [
        (
            "#[derive(Broken)]\nstruct S;\n",
            "E0282",
            "type annotations needed",
        ),
        (
            "fn a() {}\n",
            "E0277",
            "the trait bound `T: Unpin` is not satisfied",
        ),
        (
            "fn a() {}\n",
            "E0277",
            "the trait bound `T: Sync` is not satisfied",
        ),
    ] {
        let mut report = parse_items("src/lib.rs", &answers::error_at(1, 1, code, message));
        report.items[0].note = Some("existing context".into());
        set_aside_derive_expansions(&mut report, text);
        let once = report.render();
        set_aside_derive_expansions(&mut report, text);
        assert_eq!(report.render(), once);
        assert_eq!(report.errors, 1);
        assert!(
            report.items[0]
                .note
                .as_deref()
                .unwrap()
                .starts_with("existing context")
        );
        assert!(report.in_derive.is_empty() && report.auto_trait.is_empty());
        assert!(!report.ok());
    }
}
