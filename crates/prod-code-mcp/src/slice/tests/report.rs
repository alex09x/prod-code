/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::PathBuf;

use super::super::lsp::{Pos, Source, Span, Target, parse_decls, parse_locations, parse_source};
use super::super::types::{EXTERNAL_SHOWN, GAPS_SHOWN, GapKind, SliceGap, SliceItem, SliceReport};

fn span(from: (u32, u32), to: (u32, u32)) -> Span {
    Span::new(Pos::new(from.0, from.1), Pos::new(to.0, to.1))
}

fn file(path: &str) -> Source {
    Source::File(PathBuf::from(path))
}

fn item(name: &str, depth: u32, text: &str) -> SliceItem {
    SliceItem {
        file: "src/a.rs".into(),
        name: name.into(),
        kind: "function",
        start_line: 1,
        end_line: 3,
        depth,
        because: (depth > 0).then(|| "run".to_string()),
        text: text.into(),
    }
}

#[test]
fn parse_locations_reads_every_location_of_both_shapes() {
    let plain = serde_json::json!([
        { "uri": "file:///w/a.rs", "range": { "start": { "line": 4, "character": 0 }, "end": { "line": 4, "character": 3 } } },
        { "uri": "file:///w/c%20d.rs", "range": { "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 3 } } }
    ]);
    assert_eq!(
        parse_locations(&plain).unwrap(),
        vec![
            Target {
                source: file("/w/a.rs"),
                range: span((4, 0), (4, 3))
            },
            Target {
                source: file("/w/c d.rs"),
                range: span((1, 2), (1, 3))
            }
        ]
    );
    let link = serde_json::json!({ "targetUri": "file:///w/b.rs", "targetRange": { "start": { "line": 0, "character": 0 }, "end": { "line": 3, "character": 1 } }, "targetSelectionRange": { "start": { "line": 0, "character": 2 }, "end": { "line": 0, "character": 5 } } });
    assert_eq!(
        parse_locations(&link).unwrap(),
        vec![Target {
            source: file("/w/b.rs"),
            range: span((0, 2), (0, 5))
        }]
    );
    assert_eq!(parse_locations(&serde_json::json!([])).unwrap(), vec![]);
    assert_eq!(parse_locations(&serde_json::Value::Null).unwrap(), vec![]);
}

#[test]
fn parse_source_takes_local_file_uris_and_names_other_schemes() {
    for (uri, path) in [
        ("file:///w/a.rs", "/w/a.rs"),
        ("file:/w/a.rs", "/w/a.rs"),
        ("FILE:///w/a.rs", "/w/a.rs"),
        ("file://localhost/w/a.rs", "/w/a.rs"),
    ] {
        assert_eq!(parse_source(uri), Ok(file(path)), "{uri}");
    }
    for (uri, scheme) in [
        ("jdt://contents/rt.jar/java.lang/String.class", "jdt"),
        ("untitled:Untitled-1", "untitled"),
    ] {
        assert_eq!(
            parse_source(uri),
            Ok(Source::Other {
                scheme: scheme.into()
            }),
            "{uri}"
        );
    }
    for (uri, why) in [
        ("", "not an absolute URI"),
        ("src/a.rs", "not an absolute URI"),
        ("/w/a.rs", "not an absolute URI"),
        ("file:a.rs", "not an absolute file URI"),
        (" file:///w/a.rs", "not an absolute file URI"),
        ("file:///w/a.rs?x=1", "query or fragment"),
        ("file:///w/a.rs#L3", "query or fragment"),
        ("file:///w/", "names a directory"),
        ("file://", "names a directory"),
        (
            "file://buildhost/w/a.rs",
            "does not name a path on this machine",
        ),
        ("mailto:", "names nothing after its scheme"),
    ] {
        let err = parse_source(uri).expect_err(uri);
        assert!(err.contains(why), "{uri}: {err}");
    }
}

#[test]
fn parse_locations_refuses_malformed_answers() {
    for (answer, why) in [
        (serde_json::json!("src/a.rs:3"), "got a string"),
        (serde_json::json!(7), "got a number"),
        (serde_json::json!([{ "range": {} }]), "without `uri`"),
        (serde_json::json!({ "uri": 5, "range": {} }), "not a string"),
        (
            serde_json::json!({ "uri": "file:///a", "range": { "start": { "line": -1, "character": 0 }, "end": { "line": 0, "character": 0 } } }),
            "not a non-negative integer",
        ),
        (
            serde_json::json!({ "uri": "file:///a", "range": { "start": { "line": 4294967295u64, "character": 0 }, "end": { "line": 4294967295u64, "character": 0 } } }),
            "out of range",
        ),
        (
            serde_json::json!({ "uri": "file:///a", "range": { "start": { "line": 2, "character": 0 }, "end": { "line": 1, "character": 0 } } }),
            "ends before it starts",
        ),
        (
            serde_json::json!({ "uri": "file:///a" }),
            "range is missing",
        ),
        (
            serde_json::json!({ "uri": "file:///a", "range": { "end": { "line": 1, "character": 0 } } }),
            "position is missing",
        ),
    ] {
        let err = parse_locations(&answer).expect_err(why);
        assert!(err.contains(why), "{answer}: {err}");
    }
}

#[test]
fn parse_decls_reads_nested_and_flat_symbols_and_refuses_malformed_ones() {
    let nested = serde_json::json!([{
        "name": "Thing", "kind": 23,
        "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 9, "character": 1 } },
        "selectionRange": { "start": { "line": 0, "character": 11 }, "end": { "line": 0, "character": 16 } },
        "children": [
            { "name": "field", "kind": 8,
              "range": { "start": { "line": 1, "character": 4 }, "end": { "line": 1, "character": 9 } },
              "selectionRange": { "start": { "line": 1, "character": 4 }, "end": { "line": 1, "character": 9 } } },
            { "name": "run", "kind": 6,
              "range": { "start": { "line": 3, "character": 4 }, "end": { "line": 5, "character": 5 } },
              "selectionRange": { "start": { "line": 3, "character": 7 }, "end": { "line": 3, "character": 10 } },
              "children": null }
        ]
    }]);
    let decls = parse_decls(&nested).unwrap();
    let names: Vec<&str> = decls.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, vec!["Thing", "run"], "a field is not sliced");
    assert_eq!((decls[1].start_line(), decls[1].end_line()), (4, 6));

    let flat = serde_json::json!([{ "name": "CONST", "kind": 14, "location": { "uri": "file:///a", "range": { "start": { "line": 2, "character": 0 }, "end": { "line": 2, "character": 9 } } } }]);
    let decls = parse_decls(&flat).unwrap();
    assert_eq!(decls[0].selection, decls[0].range);
    assert!(parse_decls(&serde_json::Value::Null).unwrap().is_empty());

    // An inverted range like the one the real Rust engine gives `mod config;`, on a symbol
    // the slicer never slices, does not cost the file its functions.
    let module = serde_json::json!([
        { "name": "config", "kind": 2,
          "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } },
          "selectionRange": { "start": { "line": 0, "character": 8 }, "end": { "line": 0, "character": 0 } } },
        { "name": "seed", "kind": 12,
          "range": { "start": { "line": 2, "character": 0 }, "end": { "line": 4, "character": 1 } },
          "selectionRange": { "start": { "line": 2, "character": 7 }, "end": { "line": 2, "character": 11 } } }
    ]);
    let decls = parse_decls(&module).unwrap();
    assert_eq!(decls.len(), 1);
    assert_eq!(decls[0].name, "seed");

    for (answer, why) in [
        (
            serde_json::json!({ "name": "x" }),
            "expected a list of symbols",
        ),
        (serde_json::json!([{ "kind": 12 }]), "without a name"),
        (serde_json::json!([{ "name": "f" }]), "has no kind"),
        (
            serde_json::json!([{ "name": "f", "kind": 12 }]),
            "range is missing",
        ),
        (
            serde_json::json!([{ "name": "f", "kind": 12, "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 1 } }, "selectionRange": { "start": { "line": 9999999999u64, "character": 0 }, "end": { "line": 0, "character": 1 } } }]),
            "out of range",
        ),
        (
            serde_json::json!([{ "name": "f", "kind": 12, "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 1 } }, "children": {} }]),
            "not a list",
        ),
    ] {
        let err = parse_decls(&answer).expect_err(why);
        assert!(err.contains(why), "{answer}: {err}");
    }
}

#[test]
fn report_counts_bytes_and_reduction() {
    let report = SliceReport {
        seed: "run".into(),
        items: vec![item("run", 0, &"x".repeat(100))],
        source_bytes: 1000,
        external: vec!["HashMap".into(), "HashMap".into()],
        max_bytes: 120,
        truncated: 2,
        depth_limit: 2,
        unexpanded: 1,
        unsupported: vec!["String (jdt:)".into()],
        ..Default::default()
    };
    // Every lookup was answered, but the budget and the depth limit cut the walk.
    assert!(report.has_complete_evidence());
    assert!(report.is_bounded());
    assert!(!report.is_complete());
    assert_eq!(report.slice_bytes(), 100);
    assert!((report.reduction_percent() - 90.0).abs() < 0.001);
    let text = report.render();
    assert!(
        text.starts_with("BOUNDED slice of `run`: 1 item(s), 100 bytes from 1000 bytes of source (90% smaller); every dependency lookup was answered"),
        "{text}"
    );
    assert!(
        text.contains(
            "outside the workspace in a source of an unsupported URI scheme, not followed: \
             String (jdt:)\n"
        ),
        "{text}"
    );
    assert!(
        text.contains("byte budget of 120 bytes reached: 2 queued item(s) left out"),
        "{text}"
    );
    assert!(
        text.contains("depth limit 2 reached: the dependencies of 1 item(s) at depth 2"),
        "{text}"
    );
    assert!(text.contains("not followed: HashMap\n"), "{text}");
    assert!(text.contains("(the seed)"), "{text}");
    assert!(SliceReport::default().reduction_percent().abs() < f64::EPSILON);

    // Without the bounds, and with only names outside the workspace, it is complete.
    let whole = SliceReport {
        truncated: 0,
        unexpanded: 0,
        ..report
    };
    assert!(whole.is_complete());
    assert!(
        whole.render().starts_with("slice of `run`"),
        "{}",
        whole.render()
    );
}

#[test]
fn an_incomplete_report_names_each_gap_and_claims_no_reduction() {
    let gaps: Vec<SliceGap> = (0..GAPS_SHOWN + 3)
        .map(|i| SliceGap {
            kind: [
                GapKind::QueryFailed,
                GapKind::Malformed,
                GapKind::Unreadable,
                GapKind::NameLimit,
            ][i % 4],
            item: "run".into(),
            detail: format!("gap {i}"),
        })
        .collect();
    let report = SliceReport {
        seed: "run".into(),
        items: vec![item("run", 0, "fn run() {}"), item("dep", 1, "fn dep() {}")],
        source_bytes: 100,
        unsliced: (0..EXTERNAL_SHOWN + 2)
            .map(|i| format!("m{i:02} (src/m.rs:1)"))
            .collect(),
        gaps,
        max_bytes: 4,
        seed_over_budget: true,
        ..Default::default()
    };
    assert!(!report.is_complete());
    let text = report.render();
    assert!(
        text.starts_with("INCOMPLETE slice of `run`: 2 item(s), 22 bytes from 100 bytes"),
        "{text}"
    );
    assert!(!text.contains("% smaller"), "{text}");
    assert!(
        text.contains("the seed alone is 11 bytes, over the byte budget of 4 bytes"),
        "{text}"
    );
    for label in [
        "definition query failed in `run`: gap 0",
        "malformed definition answer in `run`: gap 1",
        "target cannot be sliced in `run`: gap 2",
        "name limit in `run`: gap 3",
        "  - and 3 more",
        "outside any declaration the slicer includes, not followed: m00 (src/m.rs:1)",
        ", and 2 more",
        "(depth 1, used by run)",
    ] {
        assert!(text.contains(label), "{label}: {text}");
    }
}

#[test]
fn test_render_with_dataflow_slice() {
    let code = r#"fn compute(x: i32) -> i32 {
let a = x + 1;
let unused = 99;
let b = a * 2;
b
}"#;
    let df = crate::dataflow::slice_intra_function(
        code,
        1,
        6,
        "compute",
        "src/lib.rs",
        Some(5),
        Some("b"),
    );
    let report = SliceReport {
        seed: "compute".into(),
        dataflow_slice: Some(df),
        items: vec![
            item("compute", 0, "fn compute(x: i32) -> i32 { ... }"),
            item("DepType", 1, "struct DepType;"),
        ],
        source_bytes: 200,
        ..Default::default()
    };
    let text = report.render();
    assert!(text.contains("INTRA-FUNCTION DATA-FLOW SLICE: `compute`"));
    assert!(text.contains("Completeness: COMPLETE"));
    assert!(text.contains("let a = x + 1;"));
    assert!(text.contains("let b = a * 2;"));
    assert!(!text.contains("unused"));
    assert!(text.contains("(the seed, data-flow sliced)"));
}
