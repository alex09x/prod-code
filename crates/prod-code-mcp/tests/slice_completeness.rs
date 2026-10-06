/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `slice` says what it could not establish instead of returning a smaller slice that reads as
//! complete.
//!
//! Each test drives the slicer through [`ScriptedGateway`] and checks the public result and its
//! rendered text: a failed or malformed definition answer, a target file that cannot be read, the
//! per-item name limit, the depth limit and the byte budget are all named in the answer, and a
//! walk in which the analyzer answered no definition query at all is an error. A null or empty
//! answer — a local, a keyword the scanner let through, a name that resolves nowhere — is the
//! analyzer's ordinary answer and leaves the slice complete. A location URI that names no local
//! file and a position or range the source cannot hold are malformed evidence, and a seed
//! column past its line is refused.
//!
//! The same tool against a real gateway and the real Rust engine is
//! `crates/prod-code-gateway/tests/slice_live.rs`.

use prod_code_mcp::slice;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use std::path::Path;

/// The `(line, character)` of a definition question, 0-based as the protocol sends it.
fn asked_at(params: &serde_json::Value) -> (u64, u64) {
    (
        params
            .pointer("/position/line")
            .and_then(|l| l.as_u64())
            .unwrap_or(u64::MAX),
        params
            .pointer("/position/character")
            .and_then(|c| c.as_u64())
            .unwrap_or(u64::MAX),
    )
}

/// One definition location with its start at a 0-based line and character.
fn location(path: &Path, line: u64, character: u64) -> serde_json::Value {
    serde_json::json!({
        "uri": answers::uri(path),
        "range": {
            "start": { "line": line, "character": character },
            "end": { "line": line, "character": character + 1 }
        }
    })
}

/// The reported bug: every definition query fails, yet the slice came back as the seed alone
/// with a reduction figure and no word of the failure. With no dependency evidence at all there
/// is no slice to give.
#[tokio::test]
async fn a_walk_whose_every_definition_query_failed_is_an_error_not_a_one_item_slice() {
    let ws = Workspace::new(&[(
        "src/lib.rs",
        "fn seed() { dependency(); }\nfn dependency() {}\n",
    )]);
    let root = ws.root();
    let file = root.join("src/lib.rs");
    let gateway = ScriptedGateway::start(|method, _params| match method {
        "textDocument/documentSymbol" => serde_json::json!([
            answers::document_symbol("seed", 12, 1, 1, 4),
            answers::document_symbol("dependency", 12, 2, 2, 4),
        ]),
        "textDocument/definition" => answers::failure("analyzer unavailable"),
        _ => serde_json::Value::Null,
    })
    .await;

    let result = slice::slice(gateway.addr(), &root, &file, 1, 4, 3, 4096).await;
    let rendered = result
        .as_ref()
        .map(|r| r.render())
        .unwrap_or_else(|e| format!("{e:#}"));
    assert!(
        result.is_err(),
        "no definition query was answered, yet a slice was returned:\n{rendered}"
    );
    assert!(
        rendered.contains("analyzer unavailable") && rendered.contains("no dependency evidence"),
        "{rendered}"
    );
}

/// Some queries fail, some resolve: what resolved is kept, and the slice is marked incomplete,
/// names the failed name and the analyzer's reason, and claims no reduction.
#[tokio::test]
async fn a_partly_failed_walk_is_marked_incomplete_and_keeps_what_resolved() {
    let ws = Workspace::new(&[(
        "src/lib.rs",
        "fn seed() {\n    good();\n    bad();\n}\nfn good() {}\nfn bad() {}\n",
    )]);
    let root = ws.root();
    let file = root.join("src/lib.rs");
    let target = file.clone();
    let gateway = ScriptedGateway::start(move |method, params| match method {
        "textDocument/documentSymbol" => serde_json::json!([
            answers::document_symbol("seed", 12, 1, 4, 4),
            answers::document_symbol("good", 12, 5, 5, 4),
            answers::document_symbol("bad", 12, 6, 6, 4),
        ]),
        "textDocument/definition" => match asked_at(params) {
            (1, 4) => serde_json::json!([location(&target, 4, 3)]),
            (2, 4) => answers::failure("analyzer unavailable"),
            _ => serde_json::Value::Null,
        },
        _ => serde_json::Value::Null,
    })
    .await;

    let report = slice::slice(gateway.addr(), &root, &file, 1, 4, 1, 4096)
        .await
        .expect("a partial walk still gives a slice");
    let names: Vec<&str> = report.items.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, vec!["seed", "good"], "{}", report.render());
    let text = report.render();
    assert!(text.starts_with("INCOMPLETE slice of `seed`"), "{text}");
    assert!(text.contains("`bad` at src/lib.rs:3:5"), "{text}");
    assert!(text.contains("analyzer unavailable"), "{text}");
    assert!(!text.contains("% smaller"), "{text}");
}

/// A definition answer with several locations (a name declared once per `cfg`, a trait method
/// with several bodies) pulls in every one of them, not only the first.
#[tokio::test]
async fn every_location_of_a_definition_is_followed() {
    let ws = Workspace::new(&[
        ("src/lib.rs", "fn seed() {\n    shape();\n}\n"),
        ("src/unix.rs", "pub fn shape() {}\n"),
        ("src/windows.rs", "pub fn shape() {}\n"),
    ]);
    let root = ws.root();
    let file = root.join("src/lib.rs");
    let (unix, windows) = (root.join("src/unix.rs"), root.join("src/windows.rs"));
    let gateway = ScriptedGateway::start(move |method, params| match method {
        "textDocument/documentSymbol" => {
            let uri = params
                .pointer("/textDocument/uri")
                .and_then(|u| u.as_str())
                .unwrap_or("");
            if uri.ends_with("lib.rs") {
                serde_json::json!([answers::document_symbol("seed", 12, 1, 3, 4)])
            } else {
                serde_json::json!([answers::document_symbol("shape", 12, 1, 1, 8)])
            }
        }
        "textDocument/definition" => match (
            params
                .pointer("/textDocument/uri")
                .and_then(|u| u.as_str())
                .unwrap_or("")
                .ends_with("lib.rs"),
            asked_at(params),
        ) {
            (true, (1, 4)) => {
                serde_json::json!([location(&unix, 0, 7), location(&windows, 0, 7)])
            }
            _ => serde_json::Value::Null,
        },
        _ => serde_json::Value::Null,
    })
    .await;

    let report = slice::slice(gateway.addr(), &root, &file, 1, 4, 2, 4096)
        .await
        .expect("the slice runs");
    let mut files: Vec<&str> = report.items.iter().map(|i| i.file.as_str()).collect();
    files.sort();
    assert_eq!(
        files,
        vec!["src/lib.rs", "src/unix.rs", "src/windows.rs"],
        "{}",
        report.render()
    );
    assert!(
        !report.render().contains("INCOMPLETE"),
        "{}",
        report.render()
    );
}

/// An answer that is not a location, and one whose line does not fit a coordinate, are
/// malformed evidence: named, and the slice is incomplete. Neither is read as "a local".
#[tokio::test]
async fn a_malformed_definition_answer_makes_the_slice_incomplete() {
    let ws = Workspace::new(&[(
        "src/lib.rs",
        "fn seed() {\n    good();\n    garbled();\n    huge();\n}\nfn good() {}\n",
    )]);
    let root = ws.root();
    let file = root.join("src/lib.rs");
    let target = file.clone();
    let gateway = ScriptedGateway::start(move |method, params| match method {
        "textDocument/documentSymbol" => serde_json::json!([
            answers::document_symbol("seed", 12, 1, 5, 4),
            answers::document_symbol("good", 12, 6, 6, 4),
        ]),
        "textDocument/definition" => match asked_at(params) {
            (1, 4) => serde_json::json!([location(&target, 5, 3)]),
            (2, 4) => serde_json::json!("src/lib.rs:6"),
            // 2^32: truncated to a u32 it is line 0, inside `seed`, and would pass for a local.
            (3, 4) => serde_json::json!([location(&target, 4_294_967_296, 3)]),
            _ => serde_json::Value::Null,
        },
        _ => serde_json::Value::Null,
    })
    .await;

    let report = slice::slice(gateway.addr(), &root, &file, 1, 4, 1, 4096)
        .await
        .expect("a partial walk still gives a slice");
    let text = report.render();
    assert!(text.starts_with("INCOMPLETE"), "{text}");
    assert!(text.contains("`garbled` at src/lib.rs:3:5"), "{text}");
    assert!(text.contains("`huge` at src/lib.rs:4:5"), "{text}");
    assert!(text.contains("malformed"), "{text}");
    assert!(text.contains("out of range"), "{text}");
    assert_eq!(report.items.len(), 2, "{text}");
}

/// A definition inside the workspace whose file is gone, or whose symbols the analyzer will not
/// list, cannot be sliced; the slice says so rather than dropping the dependency silently.
#[tokio::test]
async fn an_unreadable_target_file_makes_the_slice_incomplete() {
    let ws = Workspace::new(&[
        ("src/lib.rs", "fn seed() {\n    gone();\n    broken();\n}\n"),
        ("src/broken.rs", "pub fn broken() {}\n"),
    ]);
    let root = ws.root();
    let file = root.join("src/lib.rs");
    let (gone, broken) = (root.join("src/gone.rs"), root.join("src/broken.rs"));
    let gateway = ScriptedGateway::start(move |method, params| match method {
        "textDocument/documentSymbol" => {
            let uri = params
                .pointer("/textDocument/uri")
                .and_then(|u| u.as_str())
                .unwrap_or("");
            if uri.ends_with("broken.rs") {
                answers::failure("symbols unavailable")
            } else {
                serde_json::json!([answers::document_symbol("seed", 12, 1, 4, 4)])
            }
        }
        "textDocument/definition" => match asked_at(params) {
            (1, 4) => serde_json::json!([location(&gone, 0, 7)]),
            (2, 4) => serde_json::json!([location(&broken, 0, 7)]),
            _ => serde_json::Value::Null,
        },
        _ => serde_json::Value::Null,
    })
    .await;

    let report = slice::slice(gateway.addr(), &root, &file, 1, 4, 2, 4096)
        .await
        .expect("a partial walk still gives a slice");
    let text = report.render();
    assert!(text.starts_with("INCOMPLETE"), "{text}");
    assert!(text.contains("src/gone.rs:1"), "{text}");
    assert!(text.contains("src/broken.rs:1"), "{text}");
    assert!(text.contains("symbols unavailable"), "{text}");
    assert_eq!(report.items.len(), 1, "{text}");
}

/// A body naming more distinct names than the slicer resolves per item says how many were left
/// unasked, and asks exactly the limit.
#[tokio::test]
async fn a_body_over_the_name_limit_is_reported() {
    let calls: String = (0..70).map(|i| format!("    name{i:02}();\n")).collect();
    let source = format!("fn seed() {{\n{calls}}}\n");
    let ws = Workspace::new(&[("src/lib.rs", source.as_str())]);
    let root = ws.root();
    let file = root.join("src/lib.rs");
    let gateway = ScriptedGateway::start(|method, _params| match method {
        "textDocument/documentSymbol" => {
            serde_json::json!([answers::document_symbol("seed", 12, 1, 72, 4)])
        }
        _ => serde_json::Value::Null,
    })
    .await;

    let report = slice::slice(gateway.addr(), &root, &file, 1, 4, 2, 64 * 1024)
        .await
        .expect("the slice runs");
    let text = report.render();
    assert!(text.starts_with("INCOMPLETE"), "{text}");
    assert!(
        text.contains("71 distinct names; only the first 64 were resolved"),
        "{text}"
    );
    assert_eq!(gateway.calls(), 1 + 64, "one symbols query, 64 definitions");
}

/// A walk cut by the depth limit says where, and a byte budget that runs out names the budget
/// and what was left queued. Both are the caller's bounds, not missing evidence, so the slice
/// keeps its items and reduction figure; but it is not the whole dependency closure, so it is
/// marked bounded rather than complete.
#[tokio::test]
async fn the_depth_limit_and_the_byte_budget_say_where_they_cut_the_walk() {
    let seed = "fn seed() {\n    middle();\n}";
    let source = format!("{seed}\nfn middle() {{\n    leaf();\n}}\nfn leaf() {{}}\n");
    let ws = Workspace::new(&[("src/lib.rs", source.as_str())]);
    let root = ws.root();
    let file = root.join("src/lib.rs");
    let target = file.clone();
    let gateway = ScriptedGateway::start(move |method, params| match method {
        "textDocument/documentSymbol" => serde_json::json!([
            answers::document_symbol("seed", 12, 1, 3, 4),
            answers::document_symbol("middle", 12, 4, 6, 4),
            answers::document_symbol("leaf", 12, 7, 7, 4),
        ]),
        "textDocument/definition" => match asked_at(params) {
            (1, 4) => serde_json::json!([location(&target, 3, 3)]),
            (4, 4) => serde_json::json!([location(&target, 6, 3)]),
            _ => serde_json::Value::Null,
        },
        _ => serde_json::Value::Null,
    })
    .await;

    let report = slice::slice(gateway.addr(), &root, &file, 1, 4, 1, 4096)
        .await
        .expect("the slice runs");
    let text = report.render();
    assert_eq!(report.items.len(), 2, "{text}");
    assert!(report.gaps.is_empty(), "{text}");
    assert!(!report.is_complete(), "{text}");
    assert!(
        text.starts_with("BOUNDED slice of `seed`: 2 item(s)"),
        "{text}"
    );
    assert!(text.contains("% smaller"), "{text}");
    assert!(
        text.contains(
            "depth limit 1 reached: the dependencies of 1 item(s) at depth 1 were not looked up"
        ),
        "{text}"
    );

    let report = slice::slice(gateway.addr(), &root, &file, 1, 4, 3, seed.len())
        .await
        .expect("the slice runs");
    let text = report.render();
    assert_eq!(report.items.len(), 1, "{text}");
    assert!(!report.is_complete(), "{text}");
    assert!(text.starts_with("BOUNDED slice of `seed`"), "{text}");
    assert!(
        text.contains(&format!(
            "byte budget of {} bytes reached: 1 queued item(s) left out, and their own dependencies were not looked up",
            seed.len()
        )),
        "{text}"
    );

    let report = slice::slice(gateway.addr(), &root, &file, 1, 4, 0, 5)
        .await
        .expect("the slice runs");
    let text = report.render();
    assert!(
        text.contains(&format!(
            "the seed alone is {} bytes, over the byte budget of 5 bytes",
            seed.len()
        )),
        "{text}"
    );
}

/// Null and empty answers — a parameter, a local, a name that resolves nowhere — are ordinary:
/// the slice stays complete and keeps its reduction figure.
#[tokio::test]
async fn ordinary_null_and_empty_answers_leave_the_slice_complete() {
    let ws = Workspace::new(&[(
        "src/lib.rs",
        "fn seed(count: u32) -> u32 {\n    let doubled = count * 2;\n    doubled + helper()\n}\nfn helper() -> u32 {\n    1\n}\n\nfn unrelated() -> u32 {\n    2\n}\n",
    )]);
    let root = ws.root();
    let file = root.join("src/lib.rs");
    let target = file.clone();
    let gateway = ScriptedGateway::start(move |method, params| match method {
        "textDocument/documentSymbol" => serde_json::json!([
            answers::document_symbol("seed", 12, 1, 4, 4),
            answers::document_symbol("helper", 12, 5, 7, 4),
            answers::document_symbol("unrelated", 12, 9, 11, 4),
        ]),
        "textDocument/definition" => match asked_at(params) {
            (2, 14) => serde_json::json!([location(&target, 4, 3)]),
            (0, 8) | (1, 8) => serde_json::json!([]),
            _ => serde_json::Value::Null,
        },
        _ => serde_json::Value::Null,
    })
    .await;

    let report = slice::slice(gateway.addr(), &root, &file, 1, 4, 2, 4096)
        .await
        .expect("the slice runs");
    let text = report.render();
    assert!(text.starts_with("slice of `seed`: 2 item(s)"), "{text}");
    assert!(text.contains("% smaller"), "{text}");
    assert!(!text.contains("INCOMPLETE"), "{text}");
    assert!(!text.contains("missing evidence"), "{text}");
}

/// Columns go to the analyzer in UTF-16 code units, the protocol's default: a name after a
/// string with an accented letter and an emoji is asked about where it really is.
#[tokio::test]
async fn candidate_positions_are_sent_in_utf16_code_units() {
    // Bytes: `helper` starts at byte 22; UTF-16: at unit 19; the old scanner said 18.
    let ws = Workspace::new(&[(
        "src/lib.rs",
        "fn seed() {\n    let s = \"é😀\"; helper();\n}\nfn helper() {}\n",
    )]);
    let root = ws.root();
    let file = root.join("src/lib.rs");
    let target = file.clone();
    let gateway = ScriptedGateway::start(move |method, params| match method {
        "textDocument/documentSymbol" => serde_json::json!([
            answers::document_symbol("seed", 12, 1, 3, 4),
            answers::document_symbol("helper", 12, 4, 4, 4),
        ]),
        "textDocument/definition" => match asked_at(params) {
            (1, 19) => serde_json::json!([location(&target, 3, 3)]),
            _ => serde_json::Value::Null,
        },
        _ => serde_json::Value::Null,
    })
    .await;

    let report = slice::slice(gateway.addr(), &root, &file, 1, 4, 1, 4096)
        .await
        .expect("the slice runs");
    let names: Vec<&str> = report.items.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, vec!["seed", "helper"], "{}", report.render());
}

/// Two declarations on one line: a definition that lands on the second one's name slices the
/// second one, not whichever comes first.
#[tokio::test]
async fn a_definition_selects_the_declaration_whose_name_it_lands_on() {
    let ws = Workspace::new(&[(
        "src/lib.rs",
        "fn seed() {\n    beta();\n}\nfn alpha() {} fn beta() {}\n",
    )]);
    let root = ws.root();
    let file = root.join("src/lib.rs");
    let target = file.clone();
    let gateway = ScriptedGateway::start(move |method, params| match method {
        "textDocument/documentSymbol" => serde_json::json!([
            answers::document_symbol("seed", 12, 1, 3, 4),
            {
                "name": "alpha", "kind": 12,
                "range": { "start": { "line": 3, "character": 0 }, "end": { "line": 3, "character": 13 } },
                "selectionRange": { "start": { "line": 3, "character": 3 }, "end": { "line": 3, "character": 8 } }
            },
            {
                "name": "beta", "kind": 12,
                "range": { "start": { "line": 3, "character": 14 }, "end": { "line": 3, "character": 26 } },
                "selectionRange": { "start": { "line": 3, "character": 17 }, "end": { "line": 3, "character": 21 } }
            },
        ]),
        "textDocument/definition" => match asked_at(params) {
            (1, 4) => serde_json::json!([location(&target, 3, 17)]),
            _ => serde_json::Value::Null,
        },
        _ => serde_json::Value::Null,
    })
    .await;

    let report = slice::slice(gateway.addr(), &root, &file, 1, 4, 1, 4096)
        .await
        .expect("the slice runs");
    let names: Vec<&str> = report.items.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, vec!["seed", "beta"], "{}", report.render());
}

/// A seed position of line 0 is not a 1-based position; it is refused as such rather than
/// searched for.
#[tokio::test]
async fn a_zero_seed_position_is_refused_as_not_one_based() {
    let ws = Workspace::new(&[("src/lib.rs", "fn seed() {}\n")]);
    let root = ws.root();
    let file = root.join("src/lib.rs");
    let gateway = ScriptedGateway::start(|method, _params| match method {
        "textDocument/documentSymbol" => {
            serde_json::json!([answers::document_symbol("seed", 12, 1, 1, 4)])
        }
        _ => serde_json::Value::Null,
    })
    .await;

    let err = slice::slice(gateway.addr(), &root, &file, 0, 4, 1, 4096)
        .await
        .expect_err("line 0 is not a 1-based line");
    assert!(format!("{err:#}").contains("1-based"), "{err:#}");
}

/// A location's URI must name a local file: an empty or relative one, a file URI with a query,
/// another host or no leading slash is malformed evidence, not a dependency outside the
/// workspace. A URI of another scheme (a class inside a jar) is a real external source the
/// slicer cannot read; it is named as such, not turned into a path.
#[tokio::test]
async fn a_definition_uri_that_is_not_a_local_file_is_malformed_not_external() {
    let ws = Workspace::new(&[(
        "src/lib.rs",
        "fn seed() {\n    empty();\n    relative();\n    queried();\n    remote();\n    loose();\n    java();\n    good();\n}\nfn good() {}\nfn other() {}\n",
    )]);
    let root = ws.root();
    let file = root.join("src/lib.rs");
    let target = file.clone();
    let gateway = ScriptedGateway::start(move |method, params| {
        let with_uri = |uri: String| {
            serde_json::json!([{
                "uri": uri,
                "range": { "start": { "line": 10, "character": 3 }, "end": { "line": 10, "character": 8 } }
            }])
        };
        match method {
            "textDocument/documentSymbol" => serde_json::json!([
                answers::document_symbol("seed", 12, 1, 9, 4),
                answers::document_symbol("good", 12, 10, 10, 4),
                answers::document_symbol("other", 12, 11, 11, 4),
            ]),
            "textDocument/definition" => match asked_at(params) {
                (1, 4) => with_uri(String::new()),
                (2, 4) => with_uri("src/lib.rs".into()),
                (3, 4) => with_uri(format!("{}?version=2", answers::uri(&target))),
                (4, 4) => with_uri("file://buildhost/w/src/lib.rs".into()),
                (5, 4) => with_uri("file:src/lib.rs".into()),
                (6, 4) => with_uri("jdt://contents/rt.jar/java.lang/String.class".into()),
                (7, 4) => serde_json::json!([location(&target, 9, 3)]),
                _ => serde_json::Value::Null,
            },
            _ => serde_json::Value::Null,
        }
    })
    .await;

    let report = slice::slice(gateway.addr(), &root, &file, 1, 4, 1, 4096)
        .await
        .expect("a partial walk still gives a slice");
    let text = report.render();
    let mut names: Vec<&str> = report.items.iter().map(|i| i.name.as_str()).collect();
    names.sort();
    assert_eq!(
        names,
        vec!["good", "seed"],
        "a query string does not make a URI name a file: {text}"
    );
    for (name, line) in [
        ("empty", 2),
        ("relative", 3),
        ("queried", 4),
        ("remote", 5),
        ("loose", 6),
    ] {
        assert!(
            !report.external.iter().any(|e| e == name),
            "`{name}` reported as outside the workspace: {text}"
        );
        assert!(
            report.gaps.iter().any(|g| g
                .detail
                .starts_with(&format!("`{name}` at src/lib.rs:{line}:5"))),
            "`{name}` is not a named gap: {text}"
        );
    }
    assert!(
        !report.external.iter().any(|e| e == "java"),
        "a jar URI is not a path outside the workspace: {text}"
    );
    assert!(text.starts_with("INCOMPLETE"), "{text}");
    assert!(text.contains("not an absolute URI"), "{text}");
    assert!(text.contains("query"), "{text}");
    assert!(text.contains("unsupported"), "{text}");
    assert!(text.contains("java (jdt:)"), "{text}");
}

/// A definition position the source cannot hold — past the last line, past the end of a line,
/// between the two halves of a surrogate pair, or between the `\r` and `\n` of a CRLF line — is
/// malformed evidence, not a pointer to whatever declaration spans that line. A declaration whose
/// range runs past its file cannot be sliced either, rather than being cut to the lines there are.
#[tokio::test]
async fn a_definition_position_outside_its_source_is_malformed_not_a_nearby_declaration() {
    let ws = Workspace::new(&[
        (
            "src/lib.rs",
            "fn seed() {\n    past_line();\n    past_column();\n    split_pair();\n    past_crlf();\n    ghost();\n    crlf_target();\n}\nfn fine() { let _s = \"😀\"; }\n",
        ),
        ("src/crlf.rs", "pub fn crlf_target() {}\r\n"),
        ("src/ghost.rs", "pub fn ghost() {}\n"),
    ]);
    let root = ws.root();
    let file = root.join("src/lib.rs");
    let (lib, crlf, ghost) = (
        file.clone(),
        root.join("src/crlf.rs"),
        root.join("src/ghost.rs"),
    );
    let gateway = ScriptedGateway::start(move |method, params| {
        let uri = params
            .pointer("/textDocument/uri")
            .and_then(|u| u.as_str())
            .unwrap_or("");
        match method {
            "textDocument/documentSymbol" if uri.ends_with("crlf.rs") => {
                serde_json::json!([answers::document_symbol("crlf_target", 12, 1, 1, 8)])
            }
            // Thirty lines of a one-line file.
            "textDocument/documentSymbol" if uri.ends_with("ghost.rs") => {
                serde_json::json!([answers::document_symbol("ghost", 12, 1, 30, 8)])
            }
            "textDocument/documentSymbol" => serde_json::json!([
                answers::document_symbol("seed", 12, 1, 8, 4),
                answers::document_symbol("fine", 12, 9, 9, 4),
            ]),
            "textDocument/definition" => match asked_at(params) {
                (1, 4) => serde_json::json!([location(&lib, 40, 0)]),
                (2, 4) => serde_json::json!([location(&lib, 8, 200)]),
                // `fn fine() { let _s = "` is 22 units; the emoji is units 22 and 23.
                (3, 4) => serde_json::json!([location(&lib, 8, 23)]),
                // `pub fn crlf_target() {}` is 23 units; 24 is between `\r` and `\n`.
                (4, 4) => serde_json::json!([{
                    "uri": answers::uri(&crlf),
                    "range": { "start": { "line": 0, "character": 24 }, "end": { "line": 0, "character": 24 } }
                }]),
                (5, 4) => serde_json::json!([location(&ghost, 0, 7)]),
                (6, 4) => serde_json::json!([location(&crlf, 0, 7)]),
                _ => serde_json::Value::Null,
            },
            _ => serde_json::Value::Null,
        }
    })
    .await;

    let report = slice::slice(gateway.addr(), &root, &file, 1, 4, 1, 4096)
        .await
        .expect("a partial walk still gives a slice");
    let text = report.render();
    let mut names: Vec<&str> = report.items.iter().map(|i| i.name.as_str()).collect();
    names.sort();
    assert_eq!(names, vec!["crlf_target", "seed"], "{text}");
    for (name, line, why) in [
        ("past_line", 2, "past the last line"),
        ("past_column", 3, "past the end of line 9"),
        ("split_pair", 4, "splits a surrogate pair"),
        ("past_crlf", 5, "past the end of line 1"),
    ] {
        assert!(
            report.gaps.iter().any(|g| {
                g.detail
                    .starts_with(&format!("`{name}` at src/lib.rs:{line}:5"))
                    && g.detail.contains(why)
            }),
            "`{name}` is not a gap saying {why:?}: {text}"
        );
    }
    assert!(
        report
            .gaps
            .iter()
            .any(|g| g.detail.contains("src/ghost.rs") && g.detail.contains("past the last line")),
        "{text}"
    );
    assert!(report.unsliced.is_empty(), "{text}");
    assert!(text.starts_with("INCOMPLETE"), "{text}");
}

/// A seed column the line cannot hold is refused, whether it is far past the end, between the
/// `\r` and `\n` of a CRLF line, or inside a surrogate pair; a column that only names the line
/// (the first, or the end of the line) still finds the declaration spanning it.
#[tokio::test]
async fn a_seed_column_past_its_line_is_refused_while_line_only_navigation_still_works() {
    let ws = Workspace::new(&[("src/lib.rs", "fn seed() {\r\n    let _s = \"😀\";\r\n}\r\n")]);
    let root = ws.root();
    let file = root.join("src/lib.rs");
    let gateway = ScriptedGateway::start(|method, _params| match method {
        "textDocument/documentSymbol" => {
            serde_json::json!([answers::document_symbol("seed", 12, 1, 3, 4)])
        }
        _ => serde_json::Value::Null,
    })
    .await;

    // `fn seed() {` is 11 units: column 12 is its end, 13 is between `\r` and `\n`.
    // `    let _s = "` is 14 units: the emoji is columns 15 and 16.
    for (line, col, why) in [
        (1, 500, "past the end of line 1"),
        (1, 13, "past the end of line 1"),
        (2, 16, "splits a surrogate pair"),
        (40, 1, "past the last line"),
    ] {
        let err = slice::slice(gateway.addr(), &root, &file, line, col, 0, 4096)
            .await
            .map(|r| r.render())
            .expect_err(&format!("{line}:{col} is not a position in the file"));
        let err = format!("{err:#}");
        assert!(err.contains(why), "{line}:{col}: {err}");
    }
    let err = slice::slice(gateway.addr(), &root, &file, 40, 1, 0, 4096)
        .await
        .expect_err("line 40 is not in the file");
    assert!(format!("{err:#}").contains("no declaration at"), "{err:#}");

    for (line, col) in [(2, 1), (1, 12), (2, 15), (3, 2)] {
        let report = slice::slice(gateway.addr(), &root, &file, line, col, 0, 4096)
            .await
            .unwrap_or_else(|e| panic!("{line}:{col}: {e:#}"));
        assert_eq!(report.seed, "seed", "{line}:{col}");
        assert_eq!(report.items[0].text, "fn seed() {\n    let _s = \"😀\";\n}");
    }
}

/// A name that resolves into the workspace outside any declaration the slicer includes (a
/// module) is a dependency the slice does not hold: every lookup was answered, but the slice is
/// not the whole dependency closure, and it says so.
#[tokio::test]
async fn an_unsliced_target_leaves_the_slice_bounded_not_complete() {
    let ws = Workspace::new(&[(
        "src/lib.rs",
        "mod inner;\nfn seed() {\n    inner::run();\n}\n",
    )]);
    let root = ws.root();
    let file = root.join("src/lib.rs");
    let target = file.clone();
    let gateway = ScriptedGateway::start(move |method, params| match method {
        "textDocument/documentSymbol" => {
            serde_json::json!([answers::document_symbol("seed", 12, 2, 4, 4)])
        }
        "textDocument/definition" => match asked_at(params) {
            (2, 4) => serde_json::json!([location(&target, 0, 4)]),
            _ => serde_json::Value::Null,
        },
        _ => serde_json::Value::Null,
    })
    .await;

    let report = slice::slice(gateway.addr(), &root, &file, 2, 4, 3, 4096)
        .await
        .expect("the slice runs");
    let text = report.render();
    assert!(report.gaps.is_empty(), "{text}");
    assert!(!report.is_complete(), "{text}");
    assert!(
        text.starts_with("BOUNDED slice of `seed`: 1 item(s)"),
        "{text}"
    );
    assert!(text.contains("inner (src/lib.rs:1)"), "{text}");
}

/// Inverted symbol ranges (e.g. from macro expansions or synthetic type aliases, #735)
/// are normalized gracefully without erroring as "a range ends before it starts".
#[tokio::test]
async fn inverted_document_symbol_ranges_are_normalized_gracefully() {
    let ws = Workspace::new(&[(
        "src/lib.rs",
        "type LiteralStrategy = u32;\nfn seed() { let _x: LiteralStrategy = 1; }\n",
    )]);
    let root = ws.root();
    let file = root.join("src/lib.rs");
    let gateway = ScriptedGateway::start(move |method, _params| match method {
        "textDocument/documentSymbol" => {
            serde_json::json!([
                // Inverted range: line 0 character 20 down to character 5
                serde_json::json!({
                    "name": "LiteralStrategy",
                    "kind": 14,
                    "range": {
                        "start": { "line": 0, "character": 20 },
                        "end": { "line": 0, "character": 5 }
                    },
                    "selectionRange": {
                        "start": { "line": 0, "character": 20 },
                        "end": { "line": 0, "character": 5 }
                    }
                }),
                answers::document_symbol("seed", 12, 1, 1, 2)
            ])
        }
        "textDocument/definition" => serde_json::Value::Null,
        _ => serde_json::Value::Null,
    })
    .await;

    let report = slice::slice(gateway.addr(), &root, &file, 1, 3, 3, 4096)
        .await
        .expect("slice succeeds despite inverted symbol range (#735)");
    let text = report.render();
    assert!(text.contains("seed"), "{text}");
}

