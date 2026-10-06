/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::impact::diff::*;
use crate::impact::incoming::*;
use crate::impact::symbols::*;
use crate::impact::test_cmd::*;
use crate::impact::*;

#[test]
fn a_hunk_is_inside_functions_only_when_all_it_changes_is() {
    // Two functions, lines 1-3 and 5-7, a blank line between them.
    let spans = [(1, 3), (5, 7)];
    let lines = ["fn a() {", "  1", "}", "", "fn b() {", "  2", "}", "use x;"];
    let hunk = |start, added, removed| Hunk {
        start,
        added,
        removed,
    };
    assert!(hunk(2, 1, 1).inside(&spans, &lines));
    assert!(hunk(2, 1, 1).touches(1, 3));
    assert!(!hunk(2, 1, 1).touches(5, 7));
    // A rewritten line between the functions, or a new import after them, is outside.
    assert!(!hunk(4, 1, 1).inside(&spans, &lines));
    assert!(!hunk(8, 1, 0).inside(&spans, &lines));
    // A function added with the blank line that separates it: the blank changes nothing.
    assert!(hunk(4, 4, 0).inside(&spans, &lines));
    // Removed inside `a`, or removed between the two functions (a whole item gone).
    assert!(hunk(2, 0, 3).inside(&spans, &lines));
    assert!(hunk(2, 0, 3).touches(1, 3));
    assert!(!hunk(3, 0, 4).inside(&spans, &lines));
    assert!(!hunk(3, 0, 4).touches(1, 3));
    assert!(!hunk(0, 0, 2).inside(&spans, &lines));
    // Past the end of the text, nothing is left to place.
    assert!(hunk(20, 2, 0).inside(&spans, &lines));
}

#[test]
fn a_quoted_git_path_is_decoded_and_a_broken_one_is_refused() {
    assert_eq!(git_path(b"a/src/lib.rs"), Some(b"a/src/lib.rs".to_vec()));
    // A name with a space is followed by a tab that is not part of it.
    assert_eq!(git_path(b"b/sp ace.rs\t"), Some(b"b/sp ace.rs".to_vec()));
    assert_eq!(
        git_path(b"\"b/\\303\\274 x.rs\"\t"),
        Some("b/ü x.rs".as_bytes().to_vec())
    );
    assert_eq!(
        git_path(b"\"a/t\\tq\\\\\\\"z.rs\""),
        Some(b"a/t\tq\\\"z.rs".to_vec())
    );
    assert_eq!(git_path(b"\"a/open"), None);
    assert_eq!(git_path(b"\"a/\\9\""), None);
    assert_eq!(git_path(b"\"a/\\38x\""), None);
    assert_eq!(header_path(b"/dev/null", b"a/"), Some(None));
    assert_eq!(
        header_path(b"\"b/g\\303\\264ne.rs\"", b"b/"),
        Some(Some("gône.rs".to_string()))
    );
    assert_eq!(header_path(b"x/lib.rs", b"b/"), None);
}

#[test]
fn a_hunk_header_is_read_whole_or_not_at_all() {
    let hunk = |start, added, removed| Hunk {
        start,
        added,
        removed,
    };
    assert_eq!(hunk_header(b"-2 +2 @@ fn a() -1 +9"), Some(hunk(2, 1, 1)));
    assert_eq!(hunk_header(b"-1,3 +0,0 @@"), Some(hunk(0, 0, 3)));
    assert_eq!(hunk_header(b"-4,0 +5,2 @@"), Some(hunk(5, 2, 0)));
    assert_eq!(hunk_header(b"-x +2 @@"), None);
    assert_eq!(hunk_header(b"-1 +2,-1 @@"), None);
    assert_eq!(hunk_header(b"-1 +99999999999 @@"), None);
    assert_eq!(hunk_header(b"-1 +2"), None);
}

#[test]
fn a_malformed_document_symbol_is_an_error_not_a_skipped_entry() {
    let function = |line: serde_json::Value| {
        serde_json::json!({
            "name": "f", "kind": 12,
            "range": { "start": { "line": line, "character": 0 }, "end": { "line": 3, "character": 1 } },
            "selectionRange": { "start": { "line": 1, "character": 3 }, "end": { "line": 1, "character": 4 } }
        })
    };
    let mut out = Vec::new();
    assert_eq!(
        collect_functions(&[function(serde_json::json!(1))], None, &mut out),
        Ok(())
    );
    assert_eq!(out, vec![("f".to_string(), 2, 4, 2, 4)]);
    let module = |children| serde_json::json!({ "name": "m", "kind": 2, "children": children });
    for bad in [
        serde_json::json!(42),
        serde_json::json!({ "kind": 12 }),
        serde_json::json!({ "name": "", "kind": 12 }),
        serde_json::json!({ "name": "f", "kind": 0 }),
        serde_json::json!({ "name": "f", "kind": 99 }),
        serde_json::json!({ "name": "f" }),
        serde_json::json!({ "name": "f", "kind": 12 }),
        function(serde_json::json!(-1)),
        function(serde_json::json!(4_294_967_296u64)),
        function(serde_json::json!(1.5)),
        function(serde_json::json!(9)),
        module(serde_json::json!({ "x": 1 })),
        module(serde_json::json!([function(serde_json::json!("1"))])),
    ] {
        let error =
            collect_functions(std::slice::from_ref(&bad), None, &mut Vec::new()).unwrap_err();
        assert!(error.contains("cannot read"), "{bad}: {error}");
    }
    // An empty list, or a symbol with no children, is a complete answer.
    assert_eq!(collect_functions(&[], None, &mut Vec::new()), Ok(()));
    assert_eq!(
        collect_functions(&[module(serde_json::json!([]))], None, &mut Vec::new()),
        Ok(())
    );

    // JavaScript/TypeScript arrow functions and nested functions
    let js_syms = vec![
        serde_json::json!({
            "name": "handleTabCreated",
            "kind": 12,
            "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 8, "character": 1 } },
            "selectionRange": { "start": { "line": 0, "character": 16 }, "end": { "line": 0, "character": 32 } },
            "children": [
                {
                    "name": "addChildTabHandoff",
                    "kind": 12,
                    "range": { "start": { "line": 1, "character": 4 }, "end": { "line": 4, "character": 5 } },
                    "selectionRange": { "start": { "line": 1, "character": 13 }, "end": { "line": 1, "character": 31 } }
                }
            ]
        }),
        serde_json::json!({
            "name": "routeMethod",
            "kind": 14,
            "detail": "(req: any) => any",
            "range": { "start": { "line": 10, "character": 0 }, "end": { "line": 12, "character": 2 } },
            "selectionRange": { "start": { "line": 10, "character": 13 }, "end": { "line": 10, "character": 24 } }
        }),
    ];
    let mut js_out = Vec::new();
    assert_eq!(collect_functions(&js_syms, None, &mut js_out), Ok(()));
    assert_eq!(
        js_out,
        vec![
            ("handleTabCreated".to_string(), 1, 9, 1, 17),
            ("addChildTabHandoff".to_string(), 2, 5, 2, 14),
            ("routeMethod".to_string(), 11, 13, 11, 14),
        ]
    );
}

#[test]
fn an_unreadable_answer_is_quoted_short() {
    let long = serde_json::json!({ "x": "y".repeat(200) });
    let said = unreadable("m", &long);
    assert!(
        said.starts_with("m answered with something it cannot read: {"),
        "{said}"
    );
    assert!(said.ends_with('…'), "{said}");
    assert!(unreadable("m", &serde_json::json!(3)).ends_with(": 3"));
}

#[test]
fn fan_in_gap_describes_hub_and_full_suite_fallback() {
    let gap = Gap::FanIn {
        symbol: Symbol {
            name: "execute_tool".to_string(),
            file: "crates/prod-code-mcp/src/tools.rs".to_string(),
            line: 1159,
            col: 14,
        },
        callers: 82,
        limit: 30,
    };
    assert!(
        gap.describe()
            .contains("has 82 callers, exceeding the fan-in limit of 30")
    );

    let report = ImpactReport {
        language: "rust".to_string(),
        base: "HEAD".to_string(),
        changed_files: vec!["crates/prod-code-mcp/src/remote_fs.rs".to_string()],
        changed: vec![],
        callers: vec![],
        tests: (0..30)
            .map(|i| Symbol {
                name: format!("test_{i}"),
                file: "tests/suite.rs".to_string(),
                line: i as u32,
                col: 1,
            })
            .collect(),
        test_command: Some(vec![
            "cargo".to_string(),
            "test".to_string(),
            "--workspace".to_string(),
        ]),
        unattributed_files: vec![],
        index: None,
        reaches: vec![],
        incomplete: vec![gap],
        signature_warnings: vec![],
    };
    assert!(report.full_suite_reason().is_some());
    assert_eq!(report.ci_decision().run, CiRun::WholeSuite);
}

#[test]
fn test_command_falls_back_to_workspace_when_exceeding_threshold() {
    let tests: Vec<Symbol> = (0..26)
        .map(|i| Symbol {
            name: format!("test_{i}"),
            file: "tests/suite.rs".to_string(),
            line: i as u32,
            col: 1,
        })
        .collect();
    let cmd = test_command("rust", &crate::verify::ProjectTools::default(), &tests).unwrap();
    assert_eq!(cmd, vec!["cargo", "test", "--workspace"]);
}
