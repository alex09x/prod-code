//! The write tools, driven end to end against a gateway that answers from a script.
//!
//! The unit tests in each module cover the parsing and the case conversions. What they cannot
//! cover is the part where every bug in these tools has actually been: the orchestration —
//! which answer is merged with which, in what order, and what is reported when two of them
//! disagree. That needs a gateway, and a real one needs an analyzer, so here is a fake one:
//! it speaks the wire protocol and answers each LSP method from a closure the test provides.
//!
//! The scripted answers are the shapes the real engines return, including the two that must
//! not be mixed: rust-analyzer replies to a rename with the file's whole new text, while gopls
//! and the TypeScript server reply with one edit per occurrence. An early version of
//! `schema::rename` put the second phase's text edits into the same list as the first phase's
//! answer and applied them together, which turned a Rust file into duplicated fragments; the
//! two phases are separate now, and `a_whole_file_rename_is_not_merged_with_text_edits` is
//! what holds them apart.

use prod_code_testkit::{Answer, ScriptedGateway, Workspace, answers};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

/// The checkout a test drives a tool against.
fn workspace() -> Workspace {
    Workspace::empty()
}

fn write(ws: &Workspace, rel: &str, text: &str) -> PathBuf {
    ws.write(rel, text)
}

fn commit(ws: &Workspace) {
    ws.commit();
}

async fn scripted_gateway(answer: Answer) -> SocketAddr {
    ScriptedGateway::start_arc(answer).await.addr()
}

/// The file a request is about.
fn uri_of(params: &serde_json::Value) -> String {
    params
        .pointer("/textDocument/uri")
        .and_then(|u| u.as_str())
        .unwrap_or("")
        .to_string()
}

const GO: &str =
    "package backend\n\ntype Order struct {\n\tOrderID string `json:\"order_id\"`\n}\n";
const RUST: &str = "pub struct Order {\n    pub order_id: String,\n}\n\npub const Q: &str = \"SELECT order_id FROM orders\";\n";
const PROTO: &str = "message Order {\n  string order_id = 1;\n}\n";

/// The regression: a rename answered with a whole new file must not be merged with the text
/// edits of the second phase, and the string literals in that file must be found again in the
/// text the analyzer produced.
#[tokio::test]
async fn a_whole_file_rename_is_not_merged_with_text_edits() {
    let ws = workspace();
    let root = ws.root();
    let go = write(&ws, "backend/order.go", GO);
    write(
        &ws,
        "backend/go.mod",
        "module example.com/backend\n\ngo 1.22\n",
    );
    let rs = write(&ws, "core/src/lib.rs", RUST);
    write(
        &ws,
        "core/Cargo.toml",
        "[package]\nname = \"core\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(&ws, "schema/order.proto", PROTO);
    commit(&ws);

    let (go_path, rs_path) = (go.clone(), rs.clone());
    let remote = scripted_gateway(Arc::new(move |method, params| match method {
        "textDocument/rename" => {
            let uri = uri_of(params);
            if uri.ends_with("lib.rs") {
                // rust-analyzer: the whole file, already renamed, string literal untouched.
                answers::whole_file(
                    &rs_path,
                    RUST,
                    "pub struct Order {\n    pub trade_id: String,\n}\n\npub const Q: &str = \"SELECT order_id FROM orders\";\n",
                )
            } else if uri.ends_with("order.go") {
                // gopls: one edit for the identifier, and nothing for the tag.
                answers::ranged(&go_path, &[(4, 2, 7, "TradeID")])
            } else {
                serde_json::Value::Null
            }
        }
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;

    let done =
        prod_code_mcp::schema::rename(remote, &root, "order_id", "trade_id", false, false, None)
            .await
            .expect("the rename runs");

    let text_of = |name: &str| {
        done.rewritten
            .iter()
            .find(|(p, _)| p.ends_with(name))
            .map(|(_, t)| t.clone())
            .unwrap_or_else(|| panic!("{name} was not rewritten"))
    };
    // The Rust file is exactly what the analyzer returned, with only its string literal
    // rewritten on top: no duplicated fragments, no double application.
    assert_eq!(
        text_of("lib.rs"),
        "pub struct Order {\n    pub trade_id: String,\n}\n\npub const Q: &str = \"SELECT trade_id FROM orders\";\n"
    );
    // The Go file keeps the analyzer's identifier edit and gets the tag as text.
    assert_eq!(
        text_of("order.go"),
        "package backend\n\ntype Order struct {\n\tTradeID string `json:\"trade_id\"`\n}\n"
    );
    assert_eq!(
        text_of("order.proto"),
        "message Order {\n  string trade_id = 1;\n}\n"
    );
    assert!(done.diagnostics.is_empty(), "{:?}", done.diagnostics);
    assert!(!done.applied, "a dry run writes nothing");
    assert_eq!(
        std::fs::read_to_string(&rs).unwrap(),
        RUST,
        "nothing on disk changed"
    );
    assert_eq!(std::fs::read_to_string(&go).unwrap(), GO);
}

/// A file another rename replaced wholesale cannot also take a ranged edit: the ranged one was
/// computed against the file as it was, and the whole new text no longer has those positions.
#[tokio::test]
async fn a_ranged_rename_is_not_added_to_a_file_that_was_replaced_wholesale() {
    let ws = workspace();
    let root = ws.root();
    write(
        &ws,
        "Cargo.toml",
        "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    let source = "pub struct A {\n    pub order_id: String,\n}\n\npub struct B {\n    pub order_id: String,\n}\n";
    let lib = write(&ws, "src/lib.rs", source);
    commit(&ws);

    // Two structs, two different symbols: rust-analyzer answers the first with the file's
    // whole new text, and would answer the second with one too — but the first already owns
    // the file, so the second must be reported rather than applied on top.
    let path = lib.clone();
    let remote = scripted_gateway(Arc::new(move |method, params| match method {
        "textDocument/rename" => {
            let line = params
                .pointer("/position/line")
                .and_then(|l| l.as_u64())
                .unwrap_or(0);
            if line == 1 {
                answers::whole_file(
                    &path,
                    source,
                    "pub struct A {\n    pub trade_id: String,\n}\n\npub struct B {\n    pub order_id: String,\n}\n",
                )
            } else {
                answers::ranged(&path, &[(6, 9, 8, "trade_id")])
            }
        }
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;

    let done =
        prod_code_mcp::schema::rename(remote, &root, "order_id", "trade_id", false, false, None)
            .await
            .expect("the rename runs");

    let text = &done.rewritten.first().expect("one file").1;
    assert_eq!(
        text,
        "pub struct A {\n    pub trade_id: String,\n}\n\npub struct B {\n    pub order_id: String,\n}\n",
        "the second rename was not applied on top of the first one's text"
    );
    assert!(
        done.left
            .iter()
            .any(|l| l.contains("already changes these characters")),
        "the second is reported: {:?}",
        done.left
    );
}

/// Two renames that want the same characters are not merged: the second is skipped and said
/// out loud, because both were computed against the file as it is now.
#[tokio::test]
async fn a_second_rename_that_overlaps_the_first_is_reported_not_merged() {
    let ws = workspace();
    let root = ws.root();
    let ts = write(
        &ws,
        "src/order.ts",
        "export interface Order {\n  orderId: string;\n}\n\nexport function make(orderId: string): Order {\n  return { orderId };\n}\n",
    );
    write(&ws, "tsconfig.json", "{ \"include\": [\"src\"] }\n");
    write(
        &ws,
        "package.json",
        "{ \"name\": \"t\", \"version\": \"1.0.0\" }\n",
    );
    commit(&ws);

    let path = ts.clone();
    let remote = scripted_gateway(Arc::new(move |method, params| match method {
        // Both renames touch line 6: the property rename expands the shorthand, the parameter
        // rename rewrites the same characters.
        "textDocument/rename" => {
            let line = params
                .pointer("/position/line")
                .and_then(|l| l.as_u64())
                .unwrap_or(0);
            if line == 1 {
                answers::ranged(
                    &path,
                    &[(2, 3, 7, "tradeId"), (6, 12, 7, "tradeId: orderId")],
                )
            } else {
                answers::ranged(&path, &[(5, 22, 7, "tradeId"), (6, 12, 7, "tradeId")])
            }
        }
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;

    let done =
        prod_code_mcp::schema::rename(remote, &root, "order_id", "trade_id", false, false, None)
            .await
            .expect("the rename runs");

    let text = &done.rewritten.first().expect("one file").1;
    assert!(text.contains("  tradeId: string;"), "{text}");
    assert!(text.contains("return { tradeId: orderId };"), "{text}");
    // The parameter is still `orderId`: its rename collided and was left for the next run.
    assert!(
        text.contains("export function make(orderId: string)"),
        "{text}"
    );
    assert!(
        done.left
            .iter()
            .any(|l| l.contains("already changes these characters")),
        "the collision is reported: {:?}",
        done.left
    );
}

/// A field that appears only in prose is reported, never rewritten.
#[tokio::test]
async fn an_identifier_in_a_comment_is_reported_rather_than_rewritten() {
    let ws = workspace();
    let root = ws.root();
    write(
        &ws,
        "core/Cargo.toml",
        "[package]\nname = \"core\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    let rs = write(
        &ws,
        "core/src/lib.rs",
        "// the order_id is the key\npub struct Order {\n    pub symbol: String,\n}\n",
    );
    commit(&ws);

    let remote = scripted_gateway(Arc::new(move |method, _params| match method {
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;

    let done =
        prod_code_mcp::schema::rename(remote, &root, "order_id", "trade_id", false, false, None)
            .await
            .expect("the rename runs");

    assert!(
        done.rewritten.is_empty(),
        "nothing is rewritten: {:?}",
        done.rewritten
    );
    assert!(
        done.left.iter().any(|l| l.contains("lib.rs:1:8")),
        "the comment is named: {:?}",
        done.left
    );
    assert_eq!(
        std::fs::read_to_string(&rs).unwrap().lines().next(),
        Some("// the order_id is the key")
    );
}

/// `apply` writes, and only then. The same run twice: once reporting, once writing.
#[tokio::test]
async fn apply_is_what_writes_and_it_writes_everything_at_once() {
    let ws = workspace();
    let root = ws.root();
    let proto = write(&ws, "schema/order.proto", PROTO);
    let sql = write(
        &ws,
        "db/schema.sql",
        "CREATE TABLE orders (order_id TEXT);\n",
    );
    commit(&ws);

    let remote = scripted_gateway(Arc::new(move |method, _params| match method {
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;

    let report =
        prod_code_mcp::schema::rename(remote, &root, "order_id", "trade_id", false, false, None)
            .await
            .expect("the report runs");
    assert!(!report.applied);
    assert!(
        std::fs::read_to_string(&proto)
            .unwrap()
            .contains("order_id")
    );

    let done =
        prod_code_mcp::schema::rename(remote, &root, "order_id", "trade_id", true, false, None)
            .await
            .expect("the rename runs");
    assert!(done.applied);
    assert_eq!(
        std::fs::read_to_string(&proto).unwrap(),
        "message Order {\n  string trade_id = 1;\n}\n"
    );
    assert_eq!(
        std::fs::read_to_string(&sql).unwrap(),
        "CREATE TABLE orders (trade_id TEXT);\n"
    );
}

/// A result the analyzer rejects is not written, and the errors are what comes back.
#[tokio::test]
async fn a_rename_that_does_not_compile_is_not_written() {
    let ws = workspace();
    let root = ws.root();
    write(
        &ws,
        "Cargo.toml",
        "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    let lib = write(
        &ws,
        "src/lib.rs",
        "pub const Q: &str = \"SELECT order_id\";\n",
    );
    commit(&ws);

    let remote = scripted_gateway(Arc::new(move |method, _params| match method {
        "textDocument/diagnostic" => serde_json::json!({ "kind": "full", "items": [ {
            "severity": 1,
            "code": "E0308",
            "message": "mismatched types",
            "range": { "start": { "line": 0, "character": 20 }, "end": { "line": 0, "character": 28 } }
        } ] }),
        _ => serde_json::Value::Null,
    }))
    .await;

    let err =
        prod_code_mcp::schema::rename(remote, &root, "order_id", "trade_id", true, false, None)
            .await
            .expect_err("a rename that does not compile is refused");
    let text = format!("{err:#}");
    assert!(text.contains("does not compile"), "{text}");
    assert!(text.contains("E0308"), "{text}");
    assert_eq!(
        std::fs::read_to_string(&lib).unwrap(),
        "pub const Q: &str = \"SELECT order_id\";\n",
        "nothing was written"
    );
}

/// A signature change rewrites the declaration and the call sites, and says which reference it
/// could not match — the case a line-proximity check used to call done.
#[tokio::test]
async fn a_signature_change_names_the_reference_it_did_not_rewrite() {
    let ws = workspace();
    let root = ws.root();
    write(
        &ws,
        "Cargo.toml",
        "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    let source = "pub fn join(a: &str, b: &str) -> String {\n    format!(\"{a}{b}\")\n}\n\npub fn use_it() -> String {\n    join(\"x\", \"y\")\n}\n\npub fn as_a_value() -> fn(&str, &str) -> String {\n    join\n}\n";
    let lib = write(&ws, "src/lib.rs", source);
    commit(&ws);

    let path = lib.clone();
    let remote = scripted_gateway(Arc::new(move |method, _params| match method {
        // The rewrite reaches the call, not the place the function is used as a value.
        "prodCode/structuralReplace" => answers::whole_file(
            &path,
            source,
            "pub fn join(a: &str, b: &str) -> String {\n    format!(\"{a}{b}\")\n}\n\npub fn use_it() -> String {\n    join(\"y\", \"x\")\n}\n\npub fn as_a_value() -> fn(&str, &str) -> String {\n    join\n}\n",
        ),
        "textDocument/references" => serde_json::json!([
            { "uri": format!("file://{}", path.display()),
              "range": { "start": { "line": 5, "character": 4 }, "end": { "line": 5, "character": 8 } } },
            { "uri": format!("file://{}", path.display()),
              "range": { "start": { "line": 9, "character": 4 }, "end": { "line": 9, "character": 8 } } }
        ]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;

    let params = [
        prod_code_mcp::signature::parse_param("b").unwrap(),
        prod_code_mcp::signature::parse_param("a").unwrap(),
    ];
    let change =
        match prod_code_mcp::signature::change(remote, &root, &lib, 1, 8, &params, false, false)
            .await
        {
            Ok(change) => change,
            Err(err) => panic!("the change runs: {err:#}"),
        };

    assert_eq!(change.old_signature, "a: &str, b: &str");
    assert_eq!(change.new_signature, "b: &str, a: &str");
    assert_eq!(change.rule, "join($a0, $a1) ==>> join($a1, $a0)");
    let text = &change.rewritten.first().expect("one file").1;
    assert!(text.contains("pub fn join(b: &str, a: &str)"), "{text}");
    assert!(text.contains("join(\"y\", \"x\")"), "{text}");
    assert_eq!(
        change.unmatched.len(),
        1,
        "the use as a value is named: {:?}",
        change.unmatched
    );
    assert!(
        change.unmatched[0].contains("10:5"),
        "{:?}",
        change.unmatched
    );
    assert!(!change.applied);
}

/// Dropping a parameter the body still uses is refused, with the usages.
#[tokio::test]
async fn a_parameter_the_body_uses_is_not_dropped() {
    let ws = workspace();
    let root = ws.root();
    write(
        &ws,
        "Cargo.toml",
        "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    let source = "pub fn join(a: &str, b: &str) -> String {\n    format!(\"{a}{b}\")\n}\n";
    let lib = write(&ws, "src/lib.rs", source);
    commit(&ws);

    let path = lib.clone();
    let remote = scripted_gateway(Arc::new(move |method, _params| match method {
        "textDocument/references" => serde_json::json!([
            { "uri": format!("file://{}", path.display()),
              "range": { "start": { "line": 1, "character": 14 }, "end": { "line": 1, "character": 15 } } }
        ]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;

    let params = [prod_code_mcp::signature::parse_param("a").unwrap()];
    let err = prod_code_mcp::signature::change(remote, &root, &lib, 1, 8, &params, false, false)
        .await
        .expect_err("dropping a used parameter is refused");
    let text = format!("{err:#}");
    assert!(text.contains("still used by the body"), "{text}");
    assert!(text.contains('`') && text.contains('b'), "{text}");
}

/// A fixture is built from the declaration the analyzer points at, and reported as verified
/// when the analyzer accepts it.
#[tokio::test]
async fn a_fixture_is_built_from_the_declaration_and_verified() {
    let ws = workspace();
    let root = ws.root();
    write(
        &ws,
        "Cargo.toml",
        "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    let source = "pub struct Config {\n    pub name: String,\n    pub retries: u32,\n    pub verbose: bool,\n    pub tags: Vec<String>,\n}\n";
    let lib = write(&ws, "src/lib.rs", source);
    commit(&ws);

    let path = lib.clone();
    let remote = scripted_gateway(Arc::new(move |method, _params| match method {
        "workspace/symbol" => serde_json::json!([
            { "name": "Config", "kind": 23,
              "location": { "uri": format!("file://{}", path.display()),
                            "range": { "start": { "line": 0, "character": 11 },
                                       "end": { "line": 0, "character": 17 } } } }
        ]),
        "textDocument/documentSymbol" => serde_json::json!([
            { "name": "Config", "kind": 23,
              "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 5, "character": 1 } },
              "selectionRange": { "start": { "line": 0, "character": 11 }, "end": { "line": 0, "character": 17 } } }
        ]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;

    let fixture = prod_code_mcp::fixture::generate(remote, &root, "Config", 2, true, None)
        .await
        .expect("the fixture is built");

    assert_eq!(
        fixture.value,
        "Config {\n    name: String::new(),\n    retries: 0,\n    verbose: false,\n    tags: Vec::new(),\n}"
    );
    assert!(fixture.verified);
    assert!(fixture.diagnostics.is_empty(), "{:?}", fixture.diagnostics);
    assert!(
        fixture
            .render()
            .contains("the analyzer accepts it: 0 errors")
    );
}
