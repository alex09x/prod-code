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

/// The move's own orchestration: the item travels with its doc comment and the imports it
/// spells, the file it left gets one for it, and the positions the analyzer reported against
/// the file *before* the cut are adjusted for the hole the cut leaves. That last one is the
/// bug this test was written for: without the adjustment every reference below the item is
/// looked for one line too low, nothing matches, and the tool reports a move with no imports
/// at all — which compiles nowhere.
#[tokio::test]
async fn a_moved_item_takes_its_imports_and_leaves_one_behind() {
    let ws = workspace();
    let root = ws.root();
    write(
        &ws,
        "Cargo.toml",
        "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    let source = "use std::path::Path;\nuse std::collections::BTreeMap;\n\n\
                  /// Says the name.\n\
                  pub fn describe(p: &Path) -> String {\n    p.display().to_string()\n}\n\n\
                  pub fn caller(p: &Path) -> String {\n    describe(p)\n}\n";
    let lib = write(&ws, "src/lib.rs", source);
    let home = write(&ws, "src/home.rs", "//! The new home.\n");
    commit(&ws);

    let lib_for_script = lib.clone();
    let remote = scripted_gateway(Arc::new(move |method, _params| match method {
        "textDocument/documentSymbol" => serde_json::json!([
            answers::document_symbol("describe", 12, 5, 7, 8),
            answers::document_symbol("caller", 12, 9, 11, 8),
        ]),
        // `describe(p)` on line 10, as the file is before the cut.
        "textDocument/references" => answers::locations(&lib_for_script, &[(10, 5)]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;

    let moved =
        match prod_code_mcp::move_item::move_item(remote, &root, &lib, 5, 8, &home, false, false)
            .await
        {
            Ok(moved) => moved,
            Err(err) => panic!("the move runs: {err:#}"),
        };

    assert_eq!(moved.symbol, "describe");
    assert_eq!(moved.new_path, "t::home::describe");
    assert!(!moved.applied, "a move without `apply` writes nothing");
    assert_eq!(
        ws.read("src/home.rs"),
        "//! The new home.\n",
        "still on disk"
    );

    let new_home = moved
        .rewritten
        .iter()
        .find(|(p, _)| p.ends_with("home.rs"))
        .map(|(_, t)| t.clone())
        .expect("the target was rewritten");
    assert!(
        new_home.contains("/// Says the name."),
        "the doc comment travels with the item: {new_home}"
    );
    assert!(
        new_home.contains("use std::path::Path;"),
        "the import the item spells is carried: {new_home}"
    );
    assert!(
        !new_home.contains("BTreeMap"),
        "an import the item does not spell is not: {new_home}"
    );

    let left = moved
        .rewritten
        .iter()
        .find(|(p, _)| p.ends_with("lib.rs"))
        .map(|(_, t)| t.clone())
        .expect("the source was rewritten");
    assert!(
        !left.contains("pub fn describe"),
        "the item is gone from where it was: {left}"
    );
    assert!(
        left.contains("use crate::home::describe;"),
        "and the file that still calls it imports it: {left}"
    );
    assert_eq!(
        moved.imports.len(),
        2,
        "one import carried, one added: {:?}",
        moved.imports
    );
}

/// A move that would not compile is reported, not written — and the report says the thing the
/// reader needs next, which is that the item is reaching for something it can no longer see.
#[tokio::test]
async fn a_move_that_does_not_compile_is_refused_with_the_reason() {
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
        "fn secret() -> u8 {\n    1\n}\n\npub fn shown() -> u8 {\n    secret()\n}\n",
    );
    let home = write(&ws, "src/home.rs", "//! The new home.\n");
    commit(&ws);

    let home_for_script = home.clone();
    let remote = scripted_gateway(Arc::new(move |method, params| match method {
        "textDocument/documentSymbol" => serde_json::json!([
            answers::document_symbol("secret", 12, 1, 3, 4),
            answers::document_symbol("shown", 12, 5, 7, 8),
        ]),
        "textDocument/references" => serde_json::json!([]),
        "textDocument/diagnostic" if uri_of(params).ends_with("home.rs") => {
            answers::error_at(3, 5, "E0425", "cannot find function `secret` in this scope")
        }
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => {
            let _ = &home_for_script;
            serde_json::Value::Null
        }
    }))
    .await;

    let moved = prod_code_mcp::move_item::move_item(remote, &root, &lib, 5, 8, &home, false, false)
        .await
        .expect("the move runs");
    assert_eq!(moved.diagnostics.len(), 1, "{:?}", moved.diagnostics);
    let report = moved.render(4000);
    assert!(
        report.contains("the analyzer rejects the result"),
        "{report}"
    );
    assert!(
        report.contains("something private to `t`"),
        "the report explains the usual cause: {report}"
    );

    let err = prod_code_mcp::move_item::move_item(remote, &root, &lib, 5, 8, &home, true, false)
        .await
        .expect_err("apply refuses a move that does not compile");
    assert!(
        format!("{err:#}").contains("nothing was written"),
        "{err:#}"
    );
    assert_eq!(ws.read("src/home.rs"), "//! The new home.\n");
}

const HOME: &str = "pub fn build(name: &str, width: u32, height: u32) -> String {\n    let area = width * height;\n    format!(\"{name} {area}\")\n}\n\npub fn twice(name: &str) -> String {\n    build(name, 1, 2)\n}\n";
const OTHER: &str = "pub fn call_it() -> String {\n    crate::home::build(\"a\", 3, 4)\n}\n";

/// The whole shape of bundling: a struct with the declared types, a declaration that takes it,
/// a body that reaches the fields through the new binding, call sites rewritten in place, and
/// the import a caller in another module needs — which the overlay check cannot catch, because
/// the analyzer does not report an unresolved struct literal.
#[tokio::test]
async fn bundling_rewrites_the_declaration_the_body_and_every_call_site() {
    let ws = workspace();
    let root = ws.root();
    write(
        &ws,
        "Cargo.toml",
        "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(&ws, "src/lib.rs", "pub mod home;\npub mod other;\n");
    let home = write(&ws, "src/home.rs", HOME);
    let other = write(&ws, "src/other.rs", OTHER);
    commit(&ws);

    let (h, o) = (home.clone(), other.clone());
    let remote = scripted_gateway(Arc::new(move |method, params| match method {
        "textDocument/references" => {
            let line = params.pointer("/position/line").and_then(|v| v.as_u64()).unwrap_or(0);
            let ch = params
                .pointer("/position/character")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            match (line, ch) {
                // `build` itself: called once in its own file and once from the other module.
                (0, 7) => serde_json::json!([
                    { "uri": format!("file://{}", h.display()),
                      "range": { "start": { "line": 6, "character": 4 }, "end": { "line": 6, "character": 9 } } },
                    { "uri": format!("file://{}", o.display()),
                      "range": { "start": { "line": 1, "character": 17 }, "end": { "line": 1, "character": 22 } } }
                ]),
                // `width`, then `height`, each used once in the body.
                (0, 25) => answers::locations(&h, &[(2, 16)]),
                (0, 37) => answers::locations(&h, &[(2, 24)]),
                _ => serde_json::json!([]),
            }
        }
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;

    let params = ["width".to_string(), "height".to_string()];
    let done = match prod_code_mcp::parameter_object::introduce(
        remote, &root, &home, 1, 8, &params, "Size", "size", false, false,
    )
    .await
    {
        Ok(done) => done,
        Err(err) => panic!("bundling runs: {err:#}"),
    };

    assert_eq!(done.symbol, "build");
    assert_eq!(done.now, "name: &str, size: Size");
    assert_eq!(done.call_sites, 2);
    assert_eq!(done.body_uses, 2);
    assert!(done.unmatched.is_empty(), "{:?}", done.unmatched);
    assert_eq!(
        done.struct_text,
        "/// The parameters `build` takes together.\npub struct Size {\n    pub width: u32,\n    pub height: u32,\n}\n",
        "the struct keeps the declared types and needs no lifetime"
    );

    let new_home = done
        .rewritten
        .iter()
        .find(|(p, _)| p.ends_with("home.rs"))
        .map(|(_, t)| t.clone())
        .expect("the declaring file was rewritten");
    assert!(new_home.contains("pub struct Size {"), "{new_home}");
    assert!(
        new_home.contains("pub fn build(name: &str, size: Size)"),
        "{new_home}"
    );
    assert!(
        new_home.contains("let area = size.width * size.height;"),
        "the body reaches the fields through the binding: {new_home}"
    );
    assert!(
        new_home.contains("build(name, Size { width: 1, height: 2 })"),
        "the call in the same file is rewritten: {new_home}"
    );

    let new_other = done
        .rewritten
        .iter()
        .find(|(p, _)| p.ends_with("other.rs"))
        .map(|(_, t)| t.clone())
        .expect("the other module was rewritten");
    assert!(
        new_other.contains("crate::home::build(\"a\", Size { width: 3, height: 4 })"),
        "{new_other}"
    );
    assert!(
        new_other.contains("use crate::home::Size;"),
        "a caller in another module has to import the type: {new_other}"
    );
    assert!(!done.applied, "a run without `apply` writes nothing");
    assert_eq!(ws.read("src/home.rs"), HOME, "still on disk unchanged");
}

/// A use that is not a call cannot be rewritten into one, and is named rather than mangled.
#[tokio::test]
async fn a_use_that_is_not_a_call_is_reported_not_rewritten() {
    let ws = workspace();
    let root = ws.root();
    write(
        &ws,
        "Cargo.toml",
        "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(&ws, "src/lib.rs", "pub mod home;\n");
    let home = write(
        &ws,
        "src/home.rs",
        "pub fn build(name: &str, width: u32, height: u32) -> String {\n    let area = width * height;\n    format!(\"{name} {area}\")\n}\n\npub fn as_a_value() -> fn(&str, u32, u32) -> String {\n    build\n}\n",
    );
    commit(&ws);

    let h = home.clone();
    let remote = scripted_gateway(Arc::new(move |method, params| match method {
        "textDocument/references" => {
            let ch = params
                .pointer("/position/character")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            match ch {
                7 => answers::locations(&h, &[(7, 5)]), // `build` as a value, not a call
                25 => answers::locations(&h, &[(2, 16)]),
                37 => answers::locations(&h, &[(2, 24)]),
                _ => serde_json::json!([]),
            }
        }
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;

    let params = ["width".to_string(), "height".to_string()];
    let done = prod_code_mcp::parameter_object::introduce(
        remote, &root, &home, 1, 8, &params, "Size", "size", false, false,
    )
    .await
    .expect("bundling runs");

    assert_eq!(done.call_sites, 0);
    assert_eq!(done.unmatched.len(), 1, "{:?}", done.unmatched);
    assert!(
        done.unmatched[0].contains("home.rs:7:5"),
        "{:?}",
        done.unmatched
    );
    let report = done.render(4000);
    assert!(report.contains("not rewritten"), "{report}");
}

const RENDER: &str = "pub fn render(text: &str) -> String {\n    let width = 80;\n    format!(\"{text:width$}\")\n}\n\npub fn caller() -> String {\n    render(\"x\")\n}\n";

/// Extracting a magic number: the parameter is added at the end of the list, the body reads it,
/// and every existing call site passes what the body used to say — so no caller's behaviour
/// changes, which is the whole point of doing it this way round.
#[tokio::test]
async fn an_extracted_expression_becomes_the_argument_every_caller_already_passed() {
    let ws = workspace();
    let root = ws.root();
    write(
        &ws,
        "Cargo.toml",
        "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(&ws, "src/lib.rs", "pub mod home;\n");
    let home = write(&ws, "src/home.rs", RENDER);
    commit(&ws);

    let h = home.clone();
    let remote = scripted_gateway(Arc::new(move |method, _params| match method {
        "textDocument/documentSymbol" => serde_json::json!([
            answers::document_symbol("render", 12, 1, 4, 8),
            answers::document_symbol("caller", 12, 6, 8, 8),
        ]),
        "textDocument/references" => answers::locations(&h, &[(7, 5)]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;

    let done = match prod_code_mcp::extract_parameter::extract(
        remote,
        &root,
        &home,
        (2, 17),
        (2, 19),
        "width_limit",
        Some("usize"),
        false,
        false,
        false,
    )
    .await
    {
        Ok(done) => done,
        Err(err) => panic!("the extraction runs: {err:#}"),
    };

    assert_eq!(done.symbol, "render");
    assert_eq!(done.expression, "80");
    assert_eq!(done.replaced, 1);
    assert_eq!(done.call_sites, 1);
    assert!(done.unmatched.is_empty(), "{:?}", done.unmatched);

    let new_home = done
        .rewritten
        .iter()
        .find(|(p, _)| p.ends_with("home.rs"))
        .map(|(_, t)| t.clone())
        .expect("the file was rewritten");
    assert!(
        new_home.contains("pub fn render(text: &str, width_limit: usize) -> String {"),
        "{new_home}"
    );
    assert!(new_home.contains("let width = width_limit;"), "{new_home}");
    assert!(
        new_home.contains("render(\"x\", 80)"),
        "the caller passes what the body used to say: {new_home}"
    );
    assert!(!done.applied);
    assert_eq!(ws.read("src/home.rs"), RENDER, "nothing written on disk");
}

/// An expression that names something only the function can see cannot be written at a call
/// site. The check catches it, and the report says which mistake it is rather than leaving the
/// reader with a raw diagnostic.
#[tokio::test]
async fn an_expression_the_callers_cannot_see_is_refused_with_the_reason() {
    let ws = workspace();
    let root = ws.root();
    write(
        &ws,
        "Cargo.toml",
        "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(&ws, "src/lib.rs", "pub mod home;\n");
    let home = write(
        &ws,
        "src/home.rs",
        "pub fn render(text: &str) -> String {\n    let n = text.len();\n    let width = n + 2;\n    format!(\"{text:width$}\")\n}\n\npub fn caller() -> String {\n    render(\"x\")\n}\n",
    );
    commit(&ws);

    let h = home.clone();
    let remote = scripted_gateway(Arc::new(move |method, _params| match method {
        "textDocument/documentSymbol" => serde_json::json!([
            answers::document_symbol("render", 12, 1, 5, 8),
            answers::document_symbol("caller", 12, 7, 9, 8),
        ]),
        "textDocument/references" => answers::locations(&h, &[(8, 5)]),
        "textDocument/diagnostic" => {
            answers::error_at(8, 17, "E0425", "cannot find value `n` in this scope")
        }
        _ => serde_json::Value::Null,
    }))
    .await;

    let done = prod_code_mcp::extract_parameter::extract(
        remote,
        &root,
        &home,
        (3, 17),
        (3, 22),
        "width_limit",
        Some("usize"),
        false,
        false,
        false,
    )
    .await
    .expect("the extraction runs");

    assert_eq!(done.expression, "n + 2");
    assert_eq!(done.diagnostics.len(), 1, "{:?}", done.diagnostics);
    let report = done.render(4000);
    assert!(
        report.contains("if it names a local"),
        "the report names the mistake: {report}"
    );

    let err = prod_code_mcp::extract_parameter::extract(
        remote,
        &root,
        &home,
        (3, 17),
        (3, 22),
        "width_limit",
        Some("usize"),
        false,
        true,
        false,
    )
    .await
    .expect_err("apply refuses a change that does not compile");
    assert!(
        format!("{err:#}").contains("nothing was written"),
        "{err:#}"
    );
}
