//! `slice` says what it could not establish instead of returning a smaller slice that reads as
//! complete.
//!
//! Each test drives the slicer through [`ScriptedGateway`] and checks the public result and its
//! rendered text: a failed or malformed definition answer, a target file that cannot be read, the
//! per-item name limit, the depth limit and the byte budget are all named in the answer, and a
//! walk in which the analyzer answered no definition query at all is an error. A null or empty
//! answer — a local, a keyword the scanner let through, a name that resolves nowhere — is the
//! analyzer's ordinary answer and leaves the slice complete.
//!
//! The last test runs a small crate through the public `code_slice` tool against a real gateway
//! and its rust-analyzer; it needs `PROD_CODE_LIVE_GATEWAY` and fails without it.

use prod_code_mcp::slice;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use std::net::SocketAddr;
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
/// and what was left queued, without calling the slice incomplete: both are the caller's bounds.
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
    assert!(text.starts_with("slice of `seed`"), "{text}");
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

// ===== a real analyzer, through the public tool =====

const LIVE_MANIFEST: &str =
    "[package]\nname = \"slice_live\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";
const LIVE_LIB: &str = "pub mod config;\n\nuse config::Config;\n\npub fn seed(cfg: &Config) -> u32 {\n    let local = cfg.count;\n    dependency(local)\n}\n\npub fn dependency(value: u32) -> u32 {\n    value + 1\n}\n\npub fn unrelated() -> u32 {\n    7\n}\n";
const LIVE_CONFIG: &str = "pub struct Config {\n    pub count: u32,\n}\n";

/// The text of a tool's answer.
fn text_of(result: &prod_code_mcp::protocol::McpToolCallResult) -> String {
    result
        .content
        .iter()
        .map(|item| match item {
            prod_code_mcp::protocol::McpContentItem::Text { text } => text.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `code_slice` on a small crate against a real gateway: the seed, the function it calls and
/// the struct in another file it names come back, the parameter and the local resolve inside the
/// seed and cost nothing, and the slice is complete. It runs when `PROD_CODE_LIVE_GATEWAY` holds
/// the address of a running gateway; invoke this ignored test explicitly with that prerequisite.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires PROD_CODE_LIVE_GATEWAY pointing to a running gateway"]
async fn a_real_rust_analyzer_slices_a_small_crate_through_the_public_tool() {
    let addr = std::env::var("PROD_CODE_LIVE_GATEWAY")
        .expect("set PROD_CODE_LIVE_GATEWAY to run this integration test")
        .parse::<SocketAddr>()
        .expect("PROD_CODE_LIVE_GATEWAY must be a socket address");
    let ws = Workspace::new(&[
        ("Cargo.toml", LIVE_MANIFEST),
        ("src/lib.rs", LIVE_LIB),
        ("src/config.rs", LIVE_CONFIG),
    ]);
    let root = ws.root();
    let ask = || {
        prod_code_mcp::tools::execute_tool(
            addr,
            &root,
            "code_slice",
            serde_json::json!({ "path": "src/lib.rs", "line": 5, "character": 8, "depth": 3 }),
        )
    };

    // Until the analyzer has loaded the crate its answers are empty or time out, and the slice
    // says so; ask again until it is complete. Each attempt's first line is kept in the log.
    let mut last = String::new();
    for attempt in 1..=30 {
        last = match ask().await {
            Ok(result) => text_of(&result),
            Err(e) => format!("error: {e:#}"),
        };
        eprintln!(
            "attempt {attempt}: {}",
            last.lines().next().unwrap_or_default()
        );
        if last.starts_with("slice of `seed`")
            && last.contains("[function] dependency")
            && last.contains("[struct] Config")
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
    }
    eprintln!("{last}");
    assert!(last.starts_with("slice of `seed`: 3 item(s)"), "{last}");
    assert!(last.contains("[function] seed"), "{last}");
    assert!(last.contains("[function] dependency"), "{last}");
    assert!(last.contains("=== src/config.rs"), "{last}");
    assert!(last.contains("[struct] Config"), "{last}");
    assert!(!last.contains("unrelated"), "{last}");
    assert!(!last.contains("INCOMPLETE"), "{last}");
    assert!(last.contains("% smaller"), "{last}");
}
