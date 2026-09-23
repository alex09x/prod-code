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

/// The guards that refuse before anything is asked of a gateway: a name that is not an
/// identifier, an empty selection, and a selection that is in the signature rather than in the
/// body. Each one is a mistake an agent can make from a bad position, and none of them should
/// cost a round trip.
#[tokio::test]
async fn a_bad_request_is_refused_before_the_gateway_is_asked() {
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
        "pub fn render(text: &str) -> String {\n    let width = 80;\n    format!(\"{text}{width}\")\n}\n",
    );
    commit(&ws);

    // Nothing here answers anything; reaching it would hang the test rather than pass it.
    let unreachable: SocketAddr = "127.0.0.1:1".parse().expect("addr");

    let err = prod_code_mcp::extract_parameter::extract(
        unreachable,
        &root,
        &lib,
        (2, 17),
        (2, 19),
        "not an identifier",
        Some("usize"),
        false,
        false,
        false,
    )
    .await
    .expect_err("a name with spaces is not an identifier");
    assert!(
        format!("{err:#}").contains("is not an identifier"),
        "{err:#}"
    );

    let err = prod_code_mcp::extract_parameter::extract(
        unreachable,
        &root,
        &lib,
        (2, 17),
        (2, 17),
        "width_limit",
        Some("usize"),
        false,
        false,
        false,
    )
    .await
    .expect_err("an empty selection is refused");
    assert!(format!("{err:#}").contains("selection is empty"), "{err:#}");
}

/// `replace_all` and `apply` together: every identical occurrence inside the body reads the
/// parameter, the type comes from the analyzer rather than the caller, and this time the files
/// on disk actually change.
#[tokio::test]
async fn replace_all_takes_every_occurrence_and_apply_writes_them() {
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
        "pub fn render(text: &str) -> String {\n    let a = 80;\n    let b = 80;\n    format!(\"{text}{a}{b}\")\n}\n\npub fn caller() -> String {\n    render(\"x\")\n}\n",
    );
    commit(&ws);

    let h = home.clone();
    let remote = scripted_gateway(Arc::new(move |method, _params| match method {
        "textDocument/documentSymbol" => serde_json::json!([
            answers::document_symbol("render", 12, 1, 5, 8),
            answers::document_symbol("caller", 12, 7, 9, 8),
        ]),
        // The type is not given by the caller here; it comes from this.
        "textDocument/hover" => answers::hover("```rust\nlet a: usize\n```"),
        "textDocument/references" => answers::locations(&h, &[(8, 5)]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;

    let done = prod_code_mcp::extract_parameter::extract(
        remote,
        &root,
        &home,
        (2, 13),
        (2, 15),
        "width_limit",
        None,
        true,
        true,
        false,
    )
    .await
    .expect("the extraction runs");

    assert_eq!(done.ty, "usize", "the type came from the analyzer");
    assert_eq!(done.replaced, 2, "both occurrences read the parameter");
    assert!(done.applied);

    let on_disk = ws.read("src/home.rs");
    assert!(
        on_disk.contains("let a = width_limit;") && on_disk.contains("let b = width_limit;"),
        "{on_disk}"
    );
    assert!(on_disk.contains("render(\"x\", 80)"), "{on_disk}");
    assert!(
        on_disk.contains("pub fn render(text: &str, width_limit: usize)"),
        "{on_disk}"
    );
}

/// A use that is not a call cannot be given an argument, and a selection inside the signature
/// is not an expression in the body. Both are named rather than attempted.
#[tokio::test]
async fn a_value_use_gets_no_argument_and_a_selection_in_the_signature_is_refused() {
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
        "pub fn render(text: &str) -> String {\n    let width = 80;\n    format!(\"{text}{width}\")\n}\n\npub fn as_a_value() -> fn(&str) -> String {\n    render\n}\n",
    );
    commit(&ws);

    let h = home.clone();
    let remote = scripted_gateway(Arc::new(move |method, _params| match method {
        "textDocument/documentSymbol" => serde_json::json!([
            answers::document_symbol("render", 12, 1, 4, 8),
            answers::document_symbol("as_a_value", 12, 6, 8, 8),
        ]),
        "textDocument/references" => answers::locations(&h, &[(7, 5)]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;

    let done = prod_code_mcp::extract_parameter::extract(
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
    .expect("the extraction runs");
    assert_eq!(done.call_sites, 0);
    assert_eq!(done.unmatched.len(), 1, "{:?}", done.unmatched);

    let err = prod_code_mcp::extract_parameter::extract(
        remote,
        &root,
        &home,
        (1, 15),
        (1, 19),
        "width_limit",
        Some("usize"),
        false,
        false,
        false,
    )
    .await
    .expect_err("a selection in the signature is not an expression in the body");
    assert!(format!("{err:#}").contains("in the signature"), "{err:#}");
}

/// A type migration is a report, not a rewrite: the declaration moves, everything that no
/// longer fits is listed with the line of source at each site, and the two shapes of error
/// that name both types get a suggestion. Nothing is written, and `apply` refuses while any
/// site remains — a half-migrated type is worse than an unmigrated one.
#[tokio::test]
async fn a_migration_reports_the_work_and_refuses_to_write_half_of_it() {
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
        "#[derive(Debug, Clone)]\npub struct Request {\n    pub timeout_secs: u64,\n}\n\npub fn use_it(r: &Request) -> u64 {\n    r.timeout_secs\n}\n",
    );
    commit(&ws);

    // Each run pulls diagnostics twice: first for the file as it is on disk, where the derive
    // the analyzer cannot type is already there once and is not the migration's (#79), then
    // for the proposed text, where it is there twice and the second copy is.
    let pulls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let remote = scripted_gateway(Arc::new(move |method, _params| match method {
        "textDocument/references" => serde_json::json!([]),
        "textDocument/diagnostic"
            if pulls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                .is_multiple_of(2) =>
        {
            serde_json::json!({ "kind": "full", "items": [
                { "severity": 1, "code": "E0282", "message": "type annotations needed",
                  "range": { "start": { "line": 0, "character": 2 }, "end": { "line": 0, "character": 3 } } }
            ] })
        }
        "textDocument/diagnostic" => serde_json::json!({ "kind": "full", "items": [
            { "severity": 1, "code": "E0282", "message": "type annotations needed",
              "range": { "start": { "line": 0, "character": 2 }, "end": { "line": 0, "character": 3 } } },
            { "severity": 1, "code": "E0308", "message": "expected u64, found Duration",
              "range": { "start": { "line": 6, "character": 4 }, "end": { "line": 6, "character": 18 } } },
            // The same derive, reported once per expansion, on a line nobody can edit.
            { "severity": 1, "code": "E0282", "message": "type annotations needed",
              "range": { "start": { "line": 0, "character": 2 }, "end": { "line": 0, "character": 3 } } },
            { "severity": 1, "code": "E0282", "message": "type annotations needed",
              "range": { "start": { "line": 0, "character": 2 }, "end": { "line": 0, "character": 3 } } },
            { "severity": 2, "code": "unused", "message": "unused variable",
              "range": { "start": { "line": 5, "character": 4 }, "end": { "line": 5, "character": 5 } } }
        ] }),
        _ => serde_json::Value::Null,
    }))
    .await;

    let done = match prod_code_mcp::type_migration::migrate(
        remote,
        &root,
        &lib,
        3,
        9,
        "std::time::Duration",
        false,
        false,
        false,
    )
    .await
    {
        Ok(done) => done,
        Err(err) => panic!("the migration runs: {err:#}"),
    };

    assert_eq!(done.symbol, "timeout_secs");
    assert_eq!(done.was, "u64");
    assert_eq!(done.now, "std::time::Duration");
    assert_eq!(
        done.sites.len(),
        1,
        "warnings are not sites: {:?}",
        done.sites
    );
    assert_eq!(
        done.in_attributes, 1,
        "the repeated derive diagnostic is one position, set aside"
    );
    let site = &done.sites[0];
    assert_eq!(site.line, 7);
    assert_eq!(site.source, "r.timeout_secs");
    assert_eq!(
        site.suggestion.as_deref(),
        Some(
            "this place still wants `u64` and is now given `Duration`; migrate it too, or convert back here"
        )
    );

    let text = done.render(40);
    assert!(text.contains("1 site(s) in 1 file(s)"), "{text}");
    assert!(text.contains("they are the migration"), "{text}");
    assert!(text.contains("landed on a `#[derive(…)]` line"), "{text}");

    let new_text = done
        .rewritten
        .iter()
        .find(|(p, _)| p.ends_with("lib.rs"))
        .map(|(_, t)| t.clone())
        .expect("the declaration was rewritten");
    assert!(
        new_text.contains("pub timeout_secs: std::time::Duration,"),
        "{new_text}"
    );
    assert!(ws.read("src/lib.rs").contains("pub timeout_secs: u64,"));

    let err = prod_code_mcp::type_migration::migrate(
        remote,
        &root,
        &lib,
        3,
        9,
        "std::time::Duration",
        false,
        true,
        false,
    )
    .await
    .expect_err("apply refuses while a site does not fit");
    assert!(
        format!("{err:#}").contains("1 site(s) do not fit"),
        "{err:#}"
    );

    let forced = prod_code_mcp::type_migration::migrate(
        remote,
        &root,
        &lib,
        3,
        9,
        "std::time::Duration",
        false,
        true,
        true,
    )
    .await
    .expect("force writes the declaration alone");
    assert!(forced.applied);
    assert!(
        ws.read("src/lib.rs")
            .contains("pub timeout_secs: std::time::Duration,"),
        "the declaration is written and the site is not"
    );
    assert!(ws.read("src/lib.rs").contains("r.timeout_secs"));
}

/// Issue #58. A call site above the declaration, written across several lines, comes back from
/// the structural rewrite on one — so everything below it moves up, and a declaration looked
/// for again at its old line and column is not there. It never changed: it is a declaration,
/// not a call, and the rewrite does not touch it. So it is found by its own text.
#[tokio::test]
async fn a_call_above_the_declaration_that_changes_shape_does_not_lose_it() {
    let ws = workspace();
    let root = ws.root();
    write(
        &ws,
        "Cargo.toml",
        "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    let source = "pub fn caller() -> String {\n    join(\n        \"x\",\n        \"y\",\n    )\n}\n\npub fn join(a: &str, b: &str) -> String {\n    format!(\"{a}{b}\")\n}\n";
    let lib = write(&ws, "src/lib.rs", source);
    commit(&ws);

    let path = lib.clone();
    let remote = scripted_gateway(Arc::new(move |method, _params| match method {
        // The four-line call comes back as one line, as structural rewrites render them.
        "prodCode/structuralReplace" => answers::whole_file(
            &path,
            source,
            "pub fn caller() -> String {\n    join(\"y\", \"x\")\n}\n\npub fn join(a: &str, b: &str) -> String {\n    format!(\"{a}{b}\")\n}\n",
        ),
        "textDocument/references" => answers::locations(&path, &[(2, 5)]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;

    let params = [
        prod_code_mcp::signature::parse_param("b").unwrap(),
        prod_code_mcp::signature::parse_param("a").unwrap(),
    ];
    let change =
        match prod_code_mcp::signature::change(remote, &root, &lib, 8, 8, &params, false, false)
            .await
        {
            Ok(change) => change,
            Err(err) => {
                panic!("the declaration is found by its text, not its old position: {err:#}")
            }
        };
    let text = &change.rewritten.first().expect("one file").1;
    assert!(
        text.contains("pub fn join(b: &str, a: &str) -> String {"),
        "the declaration is rewritten where it now is: {text}"
    );
    assert!(text.contains("join(\"y\", \"x\")"), "{text}");
}

/// #75, from the tools' side. An analyzer that has not seen the file's latest text places a
/// reference on the wrong line. The tool looks for `(` right after where the name should end, so
/// whenever the stale position plus the name's length lands on some other call's parenthesis, the
/// argument goes into that call — `is_some_and(|g| g.applied, 2)` in this repository, where column
/// 35 plus the six letters of `render` was `is_some_and`'s `(` at 41. Here `double` is placed at the
/// stale position, the simplest way to put a parenthesis six characters on. The name has to be at
/// the position, or the position is reported and nothing there is touched.
#[tokio::test]
async fn a_position_that_does_not_hold_the_name_is_reported_not_rewritten() {
    let ws = workspace();
    let root = ws.root();
    write(
        &ws,
        "Cargo.toml",
        "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(&ws, "src/lib.rs", "pub mod home;\n");
    // `double` is six letters, like `render`; its call is where a stale position lands.
    let source = "pub fn render(text: &str) -> String {\n    let width = 80;\n    let w = double(40);\n    format!(\"{text:width$}{w}\")\n}\n\nfn double(x: u32) -> u32 {\n    x * 2\n}\n\npub fn caller() -> String {\n    render(\"x\")\n}\n";
    let home = write(&ws, "src/home.rs", source);
    commit(&ws);

    let h = home.clone();
    let remote = scripted_gateway(Arc::new(move |method, _params| match method {
        "textDocument/documentSymbol" => serde_json::json!([
            answers::document_symbol("render", 12, 1, 5, 8),
            answers::document_symbol("double", 12, 7, 9, 4),
            answers::document_symbol("caller", 12, 11, 13, 8),
        ]),
        // Stale: the call to `render` is on line 12, but this says line 3, column 13 — `double(`.
        "textDocument/references" => answers::locations(&h, &[(3, 13)]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;

    let done = prod_code_mcp::extract_parameter::extract(
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
    .expect("the extraction runs");

    let text = done
        .rewritten
        .iter()
        .find(|(p, _)| p.ends_with("home.rs"))
        .map(|(_, t)| t.clone())
        .expect("the declaration was still rewritten");
    assert!(
        text.contains("let w = double(40);"),
        "the call the stale position pointed at is untouched: {text}"
    );
    assert_eq!(
        done.call_sites, 0,
        "nothing was rewritten at the wrong place"
    );
    assert_eq!(done.unmatched.len(), 1, "{:?}", done.unmatched);
    assert!(
        done.unmatched[0].contains("the file says otherwise"),
        "{:?}",
        done.unmatched
    );
}

const CONFIG: &str = "pub struct Config {\n    pub retries: u32,\n}\n\nimpl Config {\n    pub fn new() -> Self {\n        Config { retries: 3 }\n    }\n}\n";

/// Both lists of positions in one `references` answer.
fn locations_in(files: &[(&PathBuf, &[(u32, u32)])]) -> serde_json::Value {
    serde_json::Value::Array(
        files
            .iter()
            .flat_map(|(path, spots)| {
                answers::locations(path, spots)
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
            })
            .collect(),
    )
}

/// Encapsulation end to end: outside the declaring file a read becomes a getter call and a
/// plain write a setter call; inside it the struct literal stays as it is, because a private
/// field is still visible there. The accessors go into the struct's own `impl`, with the
/// field's old visibility, and `apply` writes all of it.
#[tokio::test]
async fn encapsulating_a_field_rewrites_every_read_and_write_outside_its_file() {
    let ws = workspace();
    let root = ws.root();
    write(
        &ws,
        "Cargo.toml",
        "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(&ws, "src/lib.rs", "pub mod app;\npub mod config;\n");
    let config = write(&ws, "src/config.rs", CONFIG);
    let app = write(
        &ws,
        "src/app.rs",
        "use crate::config::Config;\n\npub fn run(cfg: &mut Config) -> u32 {\n    if cfg.retries == 0 {\n        cfg.retries = 1;\n    }\n    cfg.retries * 2\n}\n",
    );
    commit(&ws);

    let (c, a) = (config.clone(), app.clone());
    let remote = scripted_gateway(Arc::new(move |method, _params| match method {
        "textDocument/references" => {
            locations_in(&[(&c, &[(7, 18)]), (&a, &[(4, 12), (5, 13), (7, 9)])])
        }
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;

    let done = prod_code_mcp::encapsulate_field::encapsulate(
        remote, &root, &config, 2, 9, None, true, false,
    )
    .await
    .expect("the encapsulation runs");

    assert_eq!(done.owner, "Config");
    assert_eq!(done.ty, "u32");
    assert!(done.by_value, "a `u32` is returned by value");
    assert_eq!((done.reads, done.writes, done.left_in_file), (2, 1, 1));
    assert!(done.blocked.is_empty() && done.unmatched.is_empty());
    assert!(done.applied);
    assert_eq!(
        ws.read("src/app.rs"),
        "use crate::config::Config;\n\npub fn run(cfg: &mut Config) -> u32 {\n    if cfg.retries() == 0 {\n        cfg.set_retries(1);\n    }\n    cfg.retries() * 2\n}\n"
    );
    assert_eq!(
        ws.read("src/config.rs"),
        "pub struct Config {\n    retries: u32,\n}\n\nimpl Config {\n    pub fn new() -> Self {\n        Config { retries: 3 }\n    }\n\n    pub fn retries(&self) -> u32 {\n        self.retries\n    }\n\n    pub fn set_retries(&mut self, retries: u32) {\n        self.retries = retries;\n    }\n}\n"
    );
    let text = done.render(6000);
    assert!(text.contains("[applied to 2 file(s)]"), "{text}");
}

/// A use that cannot become a method call — a struct literal outside the declaring file, a
/// compound assignment, a mutable borrow — is named with its source line, and `apply` writes
/// nothing while one remains, because a private field would not compile there. A position
/// that does not hold the field's name is not touched at all.
#[tokio::test]
async fn uses_that_cannot_become_a_method_call_are_reported_and_nothing_is_written() {
    let ws = workspace();
    let root = ws.root();
    write(
        &ws,
        "Cargo.toml",
        "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(&ws, "src/lib.rs", "pub mod app;\npub mod config;\n");
    let config = write(
        &ws,
        "src/config.rs",
        "pub struct Config {\n    pub(crate) retries: u32,\n}\n",
    );
    let source = "use crate::config::Config;\n\npub fn make() -> Config {\n    Config { retries: 5 }\n}\n\npub fn bump(cfg: &mut Config) -> u32 {\n    cfg.retries += 1;\n    grow(&mut cfg.retries);\n    cfg.retries\n}\n\nfn grow(n: &mut u32) {\n    *n += 1;\n}\n";
    let app = write(&ws, "src/app.rs", source);
    commit(&ws);

    let a = app.clone();
    let remote = scripted_gateway(Arc::new(move |method, _params| match method {
        // (1, 5) is stale: `crate`, not `retries`.
        "textDocument/references" => {
            answers::locations(&a, &[(1, 5), (4, 14), (8, 9), (9, 19), (10, 9)])
        }
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;

    let done = prod_code_mcp::encapsulate_field::encapsulate(
        remote, &root, &config, 2, 16, None, false, false,
    )
    .await
    .expect("the dry run reports");
    assert_eq!(done.blocked.len(), 3, "{:?}", done.blocked);
    assert!(
        done.blocked[0].contains("struct literal")
            && done.blocked[0].contains("Config { retries: 5 }")
    );
    assert!(done.blocked[1].contains("compound assignment"));
    assert!(done.blocked[2].contains("mutable borrow"));
    assert_eq!(done.unmatched.len(), 1, "{:?}", done.unmatched);
    assert!(done.unmatched[0].contains("the file says otherwise"));
    assert_eq!(
        done.reads, 1,
        "the plain read is still rewritten in the plan"
    );
    let new_config = done
        .rewritten
        .iter()
        .find(|(p, _)| p.ends_with("config.rs"))
        .map(|(_, t)| t.clone())
        .expect("the declaring file is planned");
    assert_eq!(
        new_config,
        "pub struct Config {\n    retries: u32,\n}\n\nimpl Config {\n    pub(crate) fn retries(&self) -> u32 {\n        self.retries\n    }\n}\n",
        "a struct without an `impl` gets one, and the accessor keeps the field's visibility"
    );
    let text = done.render(6000);
    assert!(
        text.contains("3 use(s) outside the declaring file cannot become a method call"),
        "{text}"
    );

    let err = prod_code_mcp::encapsulate_field::encapsulate(
        remote, &root, &config, 2, 16, None, true, false,
    )
    .await
    .expect_err("apply refuses while a use cannot be rewritten");
    assert!(
        format!("{err:#}").contains("nothing was written"),
        "{err:#}"
    );
    assert_eq!(ws.read("src/app.rs"), source);
    assert!(ws.read("src/config.rs").contains("pub(crate) retries"));
}

/// What cannot be encapsulated is refused with the reason, before anything is planned: a
/// field that is already private, a position that is not a field, a generic struct with no
/// `impl` to put the accessors in, and a type that already has a method of the getter's name.
#[tokio::test]
async fn a_field_that_cannot_be_encapsulated_is_refused_with_the_reason() {
    let ws = workspace();
    let root = ws.root();
    let lib = write(
        &ws,
        "src/lib.rs",
        "pub struct Hidden {\n    count: u32,\n}\n\npub struct Page<T> {\n    pub rows: Vec<T>,\n}\n\npub struct Named {\n    pub label: String,\n}\n\nimpl Named {\n    pub fn label(&self) -> &str {\n        &self.label\n    }\n}\n",
    );
    commit(&ws);
    let remote = scripted_gateway(Arc::new(move |method, _params| match method {
        "textDocument/references" => serde_json::json!([]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;

    for ((line, col), expected) in [
        ((2, 5), "already private"),
        ((1, 12), "not a field declaration"),
        ((6, 9), "is generic and has no inherent `impl`"),
        ((10, 9), "already has a `fn label`"),
    ] {
        let err = prod_code_mcp::encapsulate_field::encapsulate(
            remote, &root, &lib, line, col, None, false, false,
        )
        .await
        .expect_err(expected);
        assert!(format!("{err:#}").contains(expected), "{expected}: {err:#}");
    }
}

const STORE_WITH_LIMIT: &str = "pub struct Store {\n    pub entries: Vec<u32>,\n}\n\nimpl Store {\n    pub fn new() -> Self {\n        Self { entries: Vec::new() }\n    }\n\n    pub fn limit(&self) -> usize {\n        let cap = 64 * 1024;\n        cap.min(self.entries.len())\n    }\n}\n";

/// A field extracted end to end: the method reads `self.cap`, the struct declares it last,
/// and both places that build a `Store` — `Self { … }` in its own `impl` and `Store { … }`
/// in another file, written one field per line — initialise it with the expression. A pattern
/// that ends in `..` still matches and is left alone; a return type and an import that name
/// the type are not construction sites.
#[tokio::test]
async fn extracting_a_field_initialises_it_everywhere_the_struct_is_built() {
    let ws = workspace();
    let root = ws.root();
    write(
        &ws,
        "Cargo.toml",
        "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(&ws, "src/lib.rs", "pub mod app;\npub mod store;\n");
    let store = write(&ws, "src/store.rs", STORE_WITH_LIMIT);
    let app = write(
        &ws,
        "src/app.rs",
        "use crate::store::Store;\n\npub fn make() -> Store {\n    Store {\n        entries: vec![1],\n    }\n}\n\npub fn count(s: &Store) -> usize {\n    match s {\n        Store { entries, .. } => entries.len(),\n    }\n}\n",
    );
    commit(&ws);

    let (s, a) = (store.clone(), app.clone());
    let remote = scripted_gateway(Arc::new(move |method, _params| match method {
        "textDocument/definition" => answers::locations(&s, &[(1, 12)]),
        "textDocument/references" => locations_in(&[
            (&s, &[(5, 6), (6, 21), (7, 9)]),
            (&a, &[(1, 19), (3, 18), (4, 5), (9, 18), (11, 9)]),
        ]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;

    let done = prod_code_mcp::extract_field::extract(
        remote,
        &root,
        &store,
        (11, 19),
        (11, 28),
        "cap",
        Some("usize"),
        None,
        false,
        true,
        false,
    )
    .await
    .expect("the extraction runs");

    assert_eq!(
        (done.owner.as_str(), done.method.as_str()),
        ("Store", "limit")
    );
    assert_eq!((done.replaced, done.constructors), (1, 2));
    assert!(done.blocked.is_empty(), "{:?}", done.blocked);
    assert!(done.unmatched.is_empty(), "{:?}", done.unmatched);
    assert!(done.applied);
    assert_eq!(
        ws.read("src/store.rs"),
        "pub struct Store {\n    pub entries: Vec<u32>,\n    cap: usize,\n}\n\nimpl Store {\n    pub fn new() -> Self {\n        Self { cap: 64 * 1024, entries: Vec::new() }\n    }\n\n    pub fn limit(&self) -> usize {\n        let cap = self.cap;\n        cap.min(self.entries.len())\n    }\n}\n"
    );
    assert_eq!(
        ws.read("src/app.rs"),
        "use crate::store::Store;\n\npub fn make() -> Store {\n    Store {\n        cap: 64 * 1024,\n        entries: vec![1],\n    }\n}\n\npub fn count(s: &Store) -> usize {\n    match s {\n        Store { entries, .. } => entries.len(),\n    }\n}\n"
    );
}

/// What cannot be extracted is refused with the reason: a selection outside any `impl`, a
/// method without `self`, an expression that reads `self` with no `init` to start from, a
/// field that already exists, and a tuple struct. A pattern that lists every field is named
/// with its line, and `apply` writes nothing while it remains.
#[tokio::test]
async fn a_field_that_cannot_be_extracted_is_refused_and_a_full_pattern_blocks_the_write() {
    let ws = workspace();
    let root = ws.root();
    write(
        &ws,
        "Cargo.toml",
        "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(
        &ws,
        "src/lib.rs",
        "pub mod app;\npub mod store;\npub mod pair;\n",
    );
    let store = write(&ws, "src/store.rs", STORE_WITH_LIMIT);
    let source = "use crate::store::Store;\n\npub fn size(s: &Store) -> usize {\n    let Store { entries } = s;\n    entries.len()\n}\n\npub fn free() -> usize {\n    64 * 1024\n}\n";
    let app = write(&ws, "src/app.rs", source);
    let pair = write(
        &ws,
        "src/pair.rs",
        "pub struct Pair(u8, u8);\n\nimpl Pair {\n    pub fn sum(&self) -> u8 {\n        self.0 + 1\n    }\n}\n",
    );
    commit(&ws);

    let (s, a, p) = (store.clone(), app.clone(), pair.clone());
    let remote = scripted_gateway(Arc::new(move |method, params| match method {
        "textDocument/definition" if uri_of(params).ends_with("pair.rs") => {
            answers::locations(&p, &[(1, 12)])
        }
        "textDocument/definition" => answers::locations(&s, &[(1, 12)]),
        "textDocument/references" => locations_in(&[(&s, &[(5, 6)]), (&a, &[(4, 9)])]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;

    let refused = |file: PathBuf, from: (u32, u32), to: (u32, u32), name: &'static str| {
        let root = root.clone();
        async move {
            let err = prod_code_mcp::extract_field::extract(
                remote,
                &root,
                &file,
                from,
                to,
                name,
                Some("usize"),
                None,
                false,
                false,
                false,
            )
            .await
            .expect_err("refused");
            format!("{err:#}")
        }
    };
    assert!(
        refused(app.clone(), (9, 5), (9, 14), "cap")
            .await
            .contains("not inside an `impl` block")
    );
    assert!(
        refused(store.clone(), (7, 25), (7, 35), "cap")
            .await
            .contains("takes no `self`")
    );
    assert!(
        refused(store.clone(), (12, 17), (12, 36), "cap")
            .await
            .contains("reads `self`")
    );
    assert!(
        refused(store.clone(), (11, 19), (11, 28), "entries")
            .await
            .contains("already has a field `entries`")
    );
    assert!(
        refused(pair.clone(), (5, 18), (5, 19), "one")
            .await
            .contains("not a struct with named fields")
    );

    let done = prod_code_mcp::extract_field::extract(
        remote,
        &root,
        &store,
        (11, 19),
        (11, 28),
        "cap",
        Some("usize"),
        None,
        false,
        false,
        false,
    )
    .await
    .expect("the dry run reports");
    assert_eq!(done.blocked.len(), 1, "{:?}", done.blocked);
    assert!(
        done.blocked[0].contains("src/app.rs:4:15")
            && done.blocked[0].contains("let Store { entries } = s;"),
        "{:?}",
        done.blocked
    );
    let err = prod_code_mcp::extract_field::extract(
        remote,
        &root,
        &store,
        (11, 19),
        (11, 28),
        "cap",
        Some("usize"),
        None,
        false,
        true,
        false,
    )
    .await
    .expect_err("apply refuses while a pattern would break");
    assert!(
        format!("{err:#}").contains("nothing was written"),
        "{err:#}"
    );
    assert_eq!(ws.read("src/app.rs"), source);
    assert_eq!(ws.read("src/store.rs"), STORE_WITH_LIMIT);
}

const COUNT: &str = "pub fn count(n: u32) -> u32 {\n    if n == 0 {\n        return 0;\n    }\n    count(n - 1) + 1\n}\n\npub fn twice() -> Option<u32> {\n    Some(count(2) * 2)\n}\n";
const COUNT_WRAPPED: &str = "pub fn count(n: u32) -> Option<u32> {\n    if n == 0 {\n        return Some(0);\n    }\n    Some(count(n - 1) + 1)\n}\n\npub fn twice() -> Option<u32> {\n    Some(count(2) * 2)\n}\n";

/// Wrapping a return type, end to end: rust-analyzer's assist rewrites the declaration, a caller
/// in another file that returns `Option` gets `?`, so does one later in the same file (whose
/// position the assist moved), the recursive call inside the function is left for a person, and a
/// caller that returns a plain `u32` blocks the write until `force`.
#[tokio::test]
async fn wrapping_a_return_type_propagates_where_callers_can_and_names_the_rest() {
    let ws = workspace();
    let root = ws.root();
    write(
        &ws,
        "Cargo.toml",
        "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(&ws, "src/lib.rs", "pub mod app;\npub mod count;\n");
    let count = write(&ws, "src/count.rs", COUNT);
    let app_source = "use crate::count::count;\n\npub fn more() -> Option<u32> {\n    Some(count(3) + 1)\n}\n\npub fn total() -> u32 {\n    count(4)\n}\n";
    let app = write(&ws, "src/app.rs", app_source);
    commit(&ws);

    let (c, a) = (count.clone(), app.clone());
    let remote = scripted_gateway(Arc::new(move |method, _params| match method {
        "prodCode/applyAssist" => answers::whole_file(&c, COUNT, COUNT_WRAPPED),
        "textDocument/references" => {
            locations_in(&[(&c, &[(5, 5), (9, 10)]), (&a, &[(4, 10), (8, 5)])])
        }
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;
    let run = |apply: bool, force: bool| {
        let (root, count) = (root.clone(), count.clone());
        async move {
            prod_code_mcp::wrap_return::wrap(
                remote,
                &root,
                &count,
                1,
                8,
                prod_code_mcp::wrap_return::Wrapper::Option,
                None,
                apply,
                force,
            )
            .await
        }
    };

    let done = run(false, false).await.expect("the dry run reports");
    assert_eq!(done.function, "count");
    assert_eq!(
        (done.was.as_str(), done.now.as_str()),
        ("u32", "Option<u32>")
    );
    assert_eq!(done.propagated, 2, "twice() and more()");
    assert_eq!(done.blocked.len(), 1, "{:?}", done.blocked);
    assert!(
        done.blocked[0].contains("src/app.rs:8:5") && done.blocked[0].contains("returns `u32`")
    );
    assert_eq!(done.unmatched.len(), 1, "{:?}", done.unmatched);
    assert!(done.unmatched[0].contains("inside `count` itself"));
    let new_count = done
        .rewritten
        .iter()
        .find(|(p, _)| p.ends_with("count.rs"))
        .unwrap()
        .1
        .clone();
    assert!(new_count.contains("Some(count(2)? * 2)"), "{new_count}");
    assert!(
        new_count.contains("pub fn count(n: u32) -> Option<u32>"),
        "{new_count}"
    );
    let new_app = done
        .rewritten
        .iter()
        .find(|(p, _)| p.ends_with("app.rs"))
        .unwrap()
        .1
        .clone();
    assert!(
        new_app.contains("Some(count(3)? + 1)") && new_app.contains("    count(4)\n"),
        "{new_app}"
    );

    let err = run(true, false)
        .await
        .expect_err("a blocked caller stops the write");
    assert!(
        format!("{err:#}").contains("nothing was written"),
        "{err:#}"
    );
    assert_eq!(ws.read("src/app.rs"), app_source);

    let forced = run(true, true).await.expect("force writes");
    assert!(forced.applied);
    assert!(ws.read("src/app.rs").contains("Some(count(3)? + 1)"));
}

const TWICE: &str = "pub struct S;\n\nimpl S {\n    pub fn twice(&self, x: u32) -> u32 {\n        x * 2\n    }\n\n    pub fn me(&self) -> &Self {\n        self\n    }\n}\n\npub fn a(s: &S) -> u32 {\n    s.twice(1) + S::twice(s, 2)\n}\n\npub fn b() -> u32 {\n    load().twice(3)\n}\n\npub fn load() -> S {\n    S\n}\n\npub fn c() -> fn(&S, u32) -> u32 {\n    S::twice\n}\n";

/// A method that never uses `self` becomes an associated function: the receiver leaves the
/// declaration, `s.twice(1)` becomes `S::twice(1)`, `S::twice(s, 2)` loses its first argument, a
/// receiver that runs something (`load()`) blocks the write, and the method used as a value is
/// named rather than rewritten. A method that does use `self` is refused.
#[tokio::test]
async fn a_method_that_never_uses_self_becomes_an_associated_function() {
    let ws = workspace();
    let root = ws.root();
    write(
        &ws,
        "Cargo.toml",
        "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    let lib = write(&ws, "src/lib.rs", TWICE);
    commit(&ws);
    let l = lib.clone();
    let remote = scripted_gateway(Arc::new(move |method, _params| match method {
        "textDocument/references" => {
            answers::locations(&l, &[(14, 7), (14, 21), (18, 12), (26, 8)])
        }
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;

    let done = prod_code_mcp::make_static::make_static(remote, &root, &lib, 4, 12, false, false)
        .await
        .expect("the dry run reports");
    assert_eq!(
        (
            done.owner.as_str(),
            done.method.as_str(),
            done.receiver.as_str()
        ),
        ("S", "twice", "&self")
    );
    assert_eq!(done.rewritten_calls, 2);
    assert_eq!(done.blocked.len(), 1, "{:?}", done.blocked);
    assert!(done.blocked[0].contains("`load()`"));
    assert_eq!(done.unmatched.len(), 1, "{:?}", done.unmatched);
    assert!(done.unmatched[0].contains("not a call"));
    let new = &done.rewritten[0].1;
    assert!(new.contains("pub fn twice(x: u32) -> u32"), "{new}");
    assert!(new.contains("S::twice(1) + S::twice(2)"), "{new}");
    assert!(
        new.contains("load().twice(3)"),
        "untouched while blocked: {new}"
    );

    let err = prod_code_mcp::make_static::make_static(remote, &root, &lib, 4, 12, true, false)
        .await
        .expect_err("a receiver with effects stops the write");
    assert!(
        format!("{err:#}").contains("nothing was written"),
        "{err:#}"
    );
    assert_eq!(ws.read("src/lib.rs"), TWICE);

    let refused = prod_code_mcp::make_static::make_static(remote, &root, &lib, 8, 12, false, false)
        .await
        .expect_err("`me` returns `self`");
    assert!(
        format!("{refused:#}").contains("uses `self`"),
        "{refused:#}"
    );
}

/// A method in a trait `impl` keeps its receiver: the trait decides whether it takes `self`, and
/// an implementation that dropped it would no longer implement the trait.
#[tokio::test]
async fn a_trait_impls_method_is_not_made_static() {
    let ws = workspace();
    let root = ws.root();
    let lib = write(
        &ws,
        "src/lib.rs",
        "pub struct S;\n\npub trait T {\n    fn f(&self) -> u32;\n}\n\nimpl T for S {\n    fn f(&self) -> u32 {\n        1\n    }\n}\n",
    );
    commit(&ws);
    let remote = scripted_gateway(Arc::new(|_method, _params| serde_json::Value::Null)).await;
    let err = prod_code_mcp::make_static::make_static(remote, &root, &lib, 8, 8, false, false)
        .await
        .expect_err("refused");
    assert!(
        format!("{err:#}").contains("implements a trait method"),
        "{err:#}"
    );
}

const EVEN: &str = "pub fn is_even(n: u32) -> bool {\n    if n == 0 {\n        return true;\n    }\n    let r = n % 2;\n    r == 0\n}\n\npub fn f(n: u32) -> u32 {\n    if is_even(n) && !is_even(n + 1) {\n        1\n    } else {\n        0\n    }\n}\n\npub fn g(n: u32) -> Option<u32> {\n    is_even(n).then_some(n)\n}\n\npub fn h() -> fn(u32) -> bool {\n    is_even\n}\n";

/// Inverting a predicate: the body returns the negation (its early `return` too), a call gains a
/// `!`, a call that had one loses it, a call followed by a method is parenthesized, and the function
/// used as a value is named — under its new name it would mean the opposite. A recursive predicate
/// is refused.
#[tokio::test]
async fn inverting_a_predicate_keeps_every_caller_doing_what_it_did() {
    let ws = workspace();
    let root = ws.root();
    let lib = write(&ws, "src/lib.rs", EVEN);
    commit(&ws);
    let l = lib.clone();
    let remote = scripted_gateway(Arc::new(move |method, _params| match method {
        "textDocument/references" => answers::locations(&l, &[(10, 8), (10, 23), (18, 5), (22, 5)]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;
    let done =
        prod_code_mcp::invert_boolean::invert(remote, &root, &lib, 1, 8, "is_odd", true, false)
            .await
            .expect("the inversion runs");
    assert_eq!((done.negated, done.cancelled), (2, 1));
    assert_eq!(done.unmatched.len(), 1, "{:?}", done.unmatched);
    assert!(done.unmatched[0].contains("used as a value"));
    assert!(done.applied);
    // The report of an applied edit still shows what changed (#122).
    let report = done.render(10_000);
    assert!(
        report.contains("-pub fn is_even(n: u32) -> bool {")
            && report.contains("+pub fn is_odd(n: u32) -> bool {"),
        "{report}"
    );
    let written = ws.read("src/lib.rs");
    assert!(
        written.contains("pub fn is_odd(n: u32) -> bool {\n    !{"),
        "{written}"
    );
    assert!(written.contains("return !(true);"), "{written}");
    assert!(
        written.contains("if !is_odd(n) && is_odd(n + 1) {"),
        "{written}"
    );
    assert!(written.contains("(!is_odd(n)).then_some(n)"), "{written}");

    let recursive = write(
        &ws,
        "src/rec.rs",
        "pub fn all_even(n: u32) -> bool {\n    n == 0 || all_even(n - 2)\n}\n",
    );
    let r = recursive.clone();
    let remote = scripted_gateway(Arc::new(move |method, _params| match method {
        "textDocument/references" => answers::locations(&r, &[(2, 15)]),
        _ => serde_json::Value::Null,
    }))
    .await;
    let err = prod_code_mcp::invert_boolean::invert(
        remote, &root, &recursive, 1, 8, "any_odd", false, false,
    )
    .await
    .expect_err("refused");
    assert!(format!("{err:#}").contains("calls itself"), "{err:#}");
}

const TOTAL: &str = "pub fn total(v: &Vec<u32>) -> u32 {\n    v.as_ref().iter().sum()\n}\n\npub fn pair<A: Clone>(a: A, b: String) -> (A, String) {\n    (a, b)\n}\n";

/// Making a parameter generic: its type becomes a bounded type parameter with the reference in
/// front of it kept, a function that already has generics gets one more, and the files that call it
/// are checked against the new signature — a caller the analyzer rejects stops the write.
#[tokio::test]
async fn a_generic_parameter_keeps_its_reference_and_every_caller_is_checked() {
    let ws = workspace();
    let root = ws.root();
    let lib = write(&ws, "src/lib.rs", TOTAL);
    let caller = write(
        &ws,
        "src/caller.rs",
        "pub fn g() -> u32 {\n    crate::total(&vec![1, 2])\n}\n",
    );
    commit(&ws);
    let c = caller.clone();
    let remote = scripted_gateway(Arc::new(move |method, _params| match method {
        "textDocument/references" => answers::locations(&c, &[(2, 12)]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;
    let done = prod_code_mcp::generify::generify(
        remote,
        &root,
        &lib,
        1,
        8,
        "v",
        "AsRef<[u32]>",
        "T",
        true,
        false,
    )
    .await
    .expect("the change runs");
    assert_eq!(done.was, "fn total(v: &Vec<u32>)");
    assert_eq!(done.now, "fn total<T: AsRef<[u32]>>(v: &T)");
    assert_eq!(done.callers_checked, 1);
    assert!(done.applied);
    let written = ws.read("src/lib.rs");
    assert!(
        written.contains("pub fn total<T: AsRef<[u32]>>(v: &T) -> u32 {"),
        "{written}"
    );

    let c = caller.clone();
    let remote = scripted_gateway(Arc::new(move |method, _params| match method {
        "textDocument/references" => answers::locations(&c, &[(2, 12)]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;
    let done = prod_code_mcp::generify::generify(
        remote,
        &root,
        &lib,
        5,
        8,
        "b",
        "Into<String>",
        "S",
        false,
        false,
    )
    .await
    .expect("the dry run reports");
    assert_eq!(done.now, "fn pair<A: Clone, S: Into<String>>(a: A, b: S)");
    assert!(!done.applied);

    for (param, bound, name, why) in [
        ("b", "Clone", "A", "already has a generic parameter `A`"),
        ("c", "Clone", "T", "has no parameter `c`"),
    ] {
        let remote = scripted_gateway(Arc::new(|_method, _params| serde_json::Value::Null)).await;
        let err = prod_code_mcp::generify::generify(
            remote, &root, &lib, 5, 8, param, bound, name, false, false,
        )
        .await
        .expect_err("refused");
        assert!(format!("{err:#}").contains(why), "{err:#}");
    }

    // The caller's file is clean on disk and rejected in the proposal.
    let c = caller.clone();
    let pulls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let remote = scripted_gateway(Arc::new(move |method, params| match method {
        "textDocument/references" => answers::locations(&c, &[(2, 12)]),
        "textDocument/diagnostic"
            if params.to_string().contains("caller.rs")
                && !pulls
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                    .is_multiple_of(2) =>
        {
            serde_json::json!({ "kind": "full", "items": [
                { "severity": 1, "code": "E0277", "message": "the trait bound `Vec<i32>: Into<String>` is not satisfied",
                  "range": { "start": { "line": 1, "character": 4 }, "end": { "line": 1, "character": 16 } } }
            ] })
        }
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;
    let before = ws.read("src/lib.rs");
    let err = prod_code_mcp::generify::generify(
        remote,
        &root,
        &lib,
        5,
        8,
        "b",
        "Into<String>",
        "S",
        true,
        false,
    )
    .await
    .expect_err("a caller the bound does not cover stops the write");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("nothing was written") && msg.contains("E0277"),
        "{msg}"
    );
    assert!(msg.contains("caller.rs"), "{msg}");
    assert_eq!(ws.read("src/lib.rs"), before);
}

const LIMITS: &str =
    "pub struct L {\n    pub t: u32,\n}\n\npub fn build(secs: u32) -> L {\n    L { t: secs }\n}\n";
const KEEP: &str = "pub fn keep(l: &crate::L) -> u32 {\n    l.t\n}\n";

/// A diagnostic the way rust-analyzer sends it, 1-based line, 0-based columns.
fn mismatch(code: &str, message: &str, line: u32, from: u32, to: u32) -> serde_json::Value {
    serde_json::json!({ "severity": 1, "code": code, "message": message,
        "range": { "start": { "line": line - 1, "character": from },
                   "end": { "line": line - 1, "character": to } } })
}

/// Converting a migration's sites: the widening one gets `.into()` because the analyzer accepts
/// it there, the narrowing one is tried, rejected, taken back and reported as tried, and a set of
/// conversions that breaks something elsewhere is dropped whole.
#[tokio::test]
async fn a_migration_converts_what_type_checks_and_reports_the_rest() {
    use std::collections::HashMap;
    use std::sync::Mutex;
    let ws = workspace();
    let root = ws.root();
    let lib = write(&ws, "src/lib.rs", LIMITS);
    let keep = write(&ws, "src/keep.rs", KEEP);
    commit(&ws);

    // The script plays the analyzer from the text it was sent: the field's declared type decides
    // what the two files say, and `.into()` is accepted where `From` exists (u32 -> u64) and
    // rejected where it does not (u64 -> u32).
    let texts: Arc<Mutex<HashMap<String, String>>> = Arc::new(Mutex::new(HashMap::new()));
    let breaks_elsewhere = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (k, t, b) = (
        keep.clone(),
        Arc::clone(&texts),
        Arc::clone(&breaks_elsewhere),
    );
    let script: Answer = Arc::new(move |method: &str, params: &serde_json::Value| {
        let uri = uri_of(params);
        match method {
            "textDocument/didOpen" => {
                let text = params
                    .pointer("/textDocument/text")
                    .and_then(|v| v.as_str());
                t.lock()
                    .unwrap()
                    .insert(uri, text.unwrap_or("").to_string());
                serde_json::Value::Null
            }
            "textDocument/didChange" => {
                let text = params
                    .pointer("/contentChanges/0/text")
                    .and_then(|v| v.as_str());
                t.lock()
                    .unwrap()
                    .insert(uri, text.unwrap_or("").to_string());
                serde_json::Value::Null
            }
            "textDocument/references" => answers::locations(&k, &[(2, 7)]),
            "textDocument/diagnostic" => {
                let texts = t.lock().unwrap();
                let lib_text = texts
                    .iter()
                    .find(|(u, _)| u.ends_with("src/lib.rs"))
                    .map(|(_, t)| t.clone())
                    .unwrap_or_default();
                let wide = lib_text.contains("pub t: u64");
                let this = texts.get(&uri).cloned().unwrap_or_default();
                let mut items = Vec::new();
                if uri.ends_with("src/lib.rs") && wide {
                    if this.contains("L { t: secs }") {
                        items.push(mismatch("E0308", "expected u64, found u32", 6, 11, 15));
                    }
                    if this.contains("secs.into()") && b.load(std::sync::atomic::Ordering::SeqCst) {
                        items.push(mismatch("E0282", "type annotations needed", 1, 11, 12));
                    }
                }
                if uri.ends_with("src/keep.rs") && wide {
                    if this.contains("l.t.into()") {
                        items.push(mismatch(
                            "E0277",
                            "the trait bound `u32: From<u64>` is not satisfied",
                            2,
                            4,
                            14,
                        ));
                    } else {
                        items.push(mismatch("E0308", "expected u32, found u64", 2, 4, 7));
                    }
                }
                serde_json::json!({ "kind": "full", "items": items })
            }
            _ => serde_json::Value::Null,
        }
    });
    let remote = scripted_gateway(Arc::clone(&script)).await;

    let done =
        prod_code_mcp::type_migration::migrate(remote, &root, &lib, 2, 9, "u64", true, true, true)
            .await
            .expect("the migration runs");
    assert_eq!(done.converted.len(), 1, "{:?}", done.converted);
    assert_eq!(
        (
            done.converted[0].was.as_str(),
            done.converted[0].now.as_str()
        ),
        ("secs", "secs.into()")
    );
    assert_eq!(done.sites.len(), 1, "{:?}", done.sites);
    assert_eq!(done.sites[0].file, "src/keep.rs");
    assert!(
        done.sites[0]
            .suggestion
            .as_deref()
            .is_some_and(|s| s.contains("was tried here")),
        "{:?}",
        done.sites[0]
    );
    assert!(done.applied);
    let written = ws.read("src/lib.rs");
    assert!(written.contains("pub t: u64,"), "{written}");
    assert!(written.contains("L { t: secs.into() }"), "{written}");
    assert_eq!(
        ws.read("src/keep.rs"),
        KEEP,
        "a rejected conversion is not written"
    );

    // The same conversion, when it breaks another line, is not kept.
    std::fs::write(&lib, LIMITS).unwrap();
    breaks_elsewhere.store(true, std::sync::atomic::Ordering::SeqCst);
    let remote = scripted_gateway(script).await;
    let dropped = prod_code_mcp::type_migration::migrate(
        remote, &root, &lib, 2, 9, "u64", true, false, false,
    )
    .await
    .expect("the dry run reports");
    assert!(dropped.converted.is_empty(), "{:?}", dropped.converted);
    assert!(
        dropped
            .conversion_note
            .as_deref()
            .is_some_and(|n| n.contains("cause an error elsewhere") && n.contains("src/lib.rs:1")),
        "{:?}",
        dropped.conversion_note
    );
    assert_eq!(dropped.sites.len(), 2, "{:?}", dropped.sites);
    assert!(dropped.render(10).contains("none was kept"));
}

const COUNTER: &str = "pub struct Counter {\n    pub n: u32,\n}\n\nimpl Counter {\n    pub fn bump(c: &mut Counter, by: u32) -> u32 {\n        c.n += by;\n        if by > 10 {\n            return Counter::bump(c, by - 10);\n        }\n        c.n\n    }\n}\n\npub fn run(items: &mut [Counter]) -> u32 {\n    let mut c = Counter { n: 1 };\n    Counter::bump(&mut c, 2);\n    let f = Counter::bump;\n    f(&mut c, 1) + crate::Counter::bump(&mut items[0], 3)\n}\n";

/// An associated function becomes a method: the first parameter becomes the receiver and its uses
/// in the body `self`, a path call becomes a method call on its first argument with the borrow
/// dropped, and the function used as a value and the call inside the function itself are left
/// alone, both still valid. A trait's function and a free function are refused.
#[tokio::test]
async fn an_associated_function_becomes_a_method_with_every_call() {
    let ws = workspace();
    let root = ws.root();
    let lib = write(&ws, "src/lib.rs", COUNTER);
    commit(&ws);
    let l = lib.clone();
    let remote = scripted_gateway(Arc::new(move |method, params| {
        let character = params
            .pointer("/position/character")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        match method {
            // The parameter `c` (6:17): its uses in the body, one of them in the recursive call.
            "textDocument/references" if character == 16 => {
                answers::locations(&l, &[(7, 9), (9, 34), (11, 9)])
            }
            // The function `bump`: the recursive call, two path calls and one use as a value.
            "textDocument/references" => {
                answers::locations(&l, &[(9, 29), (17, 14), (18, 22), (19, 36)])
            }
            "textDocument/diagnostic" => answers::no_diagnostics(),
            _ => serde_json::Value::Null,
        }
    }))
    .await;
    let done = prod_code_mcp::to_method::convert_to_method(remote, &root, &lib, 6, 12, true, false)
        .await
        .expect("the change runs");
    assert_eq!(
        (done.parameter.as_str(), done.receiver.as_str()),
        ("c: &mut Counter", "&mut self")
    );
    assert_eq!(done.renamed_uses, 3);
    assert_eq!(done.rewritten_calls, 2);
    assert_eq!(done.unchanged.len(), 2, "{:?}", done.unchanged);
    assert!(done.unchanged.iter().any(|u| u.contains("used as a value")));
    assert!(
        done.unchanged
            .iter()
            .any(|u| u.contains("inside `bump` itself"))
    );
    assert!(done.applied);
    let written = ws.read("src/lib.rs");
    assert!(
        written.contains("pub fn bump(&mut self, by: u32) -> u32 {\n        self.n += by;"),
        "{written}"
    );
    assert!(
        written.contains("return Counter::bump(self, by - 10);"),
        "{written}"
    );
    assert!(written.contains("\n    c.bump(2);\n"), "{written}");
    assert!(written.contains("let f = Counter::bump;"), "{written}");
    assert!(written.contains("items[0].bump(3)"), "{written}");

    let refused = write(
        &ws,
        "src/other.rs",
        "pub struct S;\n\npub trait T {\n    fn f(s: &S);\n}\n\nimpl T for S {\n    fn f(s: &S) {}\n}\n\npub fn free(s: &S) {}\n\nimpl S {\n    pub fn g(x: u32) {}\n}\n",
    );
    for (line, col, why) in [
        (8, 8, "implements a trait function"),
        (11, 8, "not inside an `impl` block"),
        (14, 12, "is not `S`, `&S` or `&mut S`"),
    ] {
        let remote = scripted_gateway(Arc::new(|_method, _params| serde_json::Value::Null)).await;
        let err = prod_code_mcp::to_method::convert_to_method(
            remote, &root, &refused, line, col, false, false,
        )
        .await
        .expect_err("refused");
        assert!(format!("{err:#}").contains(why), "{err:#}");
    }
}

const FLAGS: &str = "pub struct Flags {\n    pub enabled: bool,\n    pub n: u32,\n}\n\npub fn make(on: bool) -> Flags {\n    Flags { enabled: on, n: 1 }\n}\n\npub fn read(f: &Flags) -> u32 {\n    if f.enabled && !f.enabled { 1 } else { f.enabled.then_some(2).unwrap_or(0) }\n}\n\npub fn write(f: &mut Flags, v: bool) {\n    f.enabled = v;\n}\n";

const DEFAULTED: &str = "#[derive(Default)]\npub struct G {\n    pub on: bool,\n}\n\npub fn h(g: &mut G) -> bool {\n    let r = &g.on;\n    g.on |= true;\n    let G { on } = g;\n    *r || *on\n}\n";

const SMALL: &str = "pub fn f(x: u32) -> bool {\n    let mut small = x < 10;\n    if x == 0 {\n        small = false;\n    }\n    let early = if x > 5 { small } else { false };\n    small && early\n}\n";

/// Inverting a boolean field: every read gains a `!` or loses the one it had, a read that goes on
/// is parenthesized, every write and the struct literal store the negation. A borrow, a compound
/// assignment, a pattern and a derived `Default` cannot keep their meaning, and block the write.
/// A local is inverted the same way once the analyzer says it is a `bool`, and refused when not.
#[tokio::test]
async fn a_boolean_field_or_local_is_inverted_with_every_read_and_write() {
    let ws = workspace();
    let root = ws.root();
    let lib = write(&ws, "src/lib.rs", FLAGS);
    commit(&ws);
    let l = lib.clone();
    let remote = scripted_gateway(Arc::new(move |method, _params| match method {
        "textDocument/references" => {
            answers::locations(&l, &[(7, 13), (11, 10), (11, 24), (11, 47), (15, 7)])
        }
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;
    let done =
        prod_code_mcp::invert_boolean::invert(remote, &root, &lib, 2, 9, "disabled", true, false)
            .await
            .expect("the field is inverted");
    assert_eq!(done.kind, "field");
    assert_eq!((done.negated, done.cancelled, done.writes), (2, 1, 2));
    assert!(done.blocked.is_empty(), "{:?}", done.blocked);
    assert!(done.applied);
    let written = ws.read("src/lib.rs");
    for expected in [
        "    pub disabled: bool,",
        "Flags { disabled: !(on), n: 1 }",
        "if !f.disabled && f.disabled { 1 } else { (!f.disabled).then_some(2).unwrap_or(0) }",
        "    f.disabled = !(v);",
    ] {
        assert!(written.contains(expected), "{expected}\n{written}");
    }

    let g = write(&ws, "src/g.rs", DEFAULTED);
    let gp = g.clone();
    let remote = scripted_gateway(Arc::new(move |method, _params| match method {
        "textDocument/references" => answers::locations(&gp, &[(7, 16), (8, 7), (9, 13)]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;
    let blocked =
        prod_code_mcp::invert_boolean::invert(remote, &root, &g, 3, 9, "off", false, false)
            .await
            .expect("the dry run reports");
    let reasons = blocked.blocked.join("\n");
    assert_eq!(blocked.blocked.len(), 4, "{reasons}");
    for why in [
        "derives `Default`",
        "is borrowed",
        "compound assignment",
        "a pattern binds",
    ] {
        assert!(reasons.contains(why), "{why}\n{reasons}");
    }
    let remote = scripted_gateway(Arc::new(|_method, _params| serde_json::Value::Null)).await;
    let err = prod_code_mcp::invert_boolean::invert(remote, &root, &g, 3, 9, "off", true, false)
        .await
        .expect_err("a blocked use stops the write");
    assert!(format!("{err:#}").contains("nothing was"), "{err:#}");
    assert_eq!(ws.read("src/g.rs"), DEFAULTED);

    let small = write(&ws, "src/small.rs", SMALL);
    let (sp, hover) = (small.clone(), Arc::new(std::sync::Mutex::new("bool")));
    let h = Arc::clone(&hover);
    let remote = scripted_gateway(Arc::new(move |method, _params| match method {
        "textDocument/hover" => serde_json::json!({ "contents": { "kind": "markdown",
            "value": format!("```rust\nlet mut small: {}\n```", h.lock().unwrap()) } }),
        "textDocument/references" => answers::locations(&sp, &[(4, 9), (6, 28), (7, 5)]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;
    let done =
        prod_code_mcp::invert_boolean::invert(remote, &root, &small, 2, 13, "large", true, false)
            .await
            .expect("the local is inverted");
    assert_eq!(done.kind, "variable");
    assert_eq!((done.negated, done.writes), (2, 2));
    let written = ws.read("src/small.rs");
    for expected in [
        "let mut large = !(x < 10);",
        "        large = true;",
        "let early = if x > 5 { !large } else { false };",
        "    !large && early\n",
    ] {
        assert!(written.contains(expected), "{expected}\n{written}");
    }

    *hover.lock().unwrap() = "u32";
    std::fs::write(&small, SMALL).unwrap();
    let err =
        prod_code_mcp::invert_boolean::invert(remote, &root, &small, 2, 13, "large", false, false)
            .await
            .expect_err("a u32 is not a boolean");
    assert!(format!("{err:#}").contains("`u32`, not `bool`"), "{err:#}");
}

const JOIN: &str = "pub fn join(a: &str, b: &str) -> String {\n    a.to_string()\n}\n\npub fn use_it() -> String {\n    join(\"x\", \"y\")\n}\n";

/// Safe-deleting a parameter removes it from the declaration and its argument from every call,
/// and is refused while the body still uses it.
#[tokio::test]
async fn safe_delete_at_a_parameter_removes_it_with_its_arguments() {
    let ws = workspace();
    let root = ws.root();
    write(
        &ws,
        "Cargo.toml",
        "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    let lib = write(&ws, "src/lib.rs", JOIN);
    commit(&ws);
    let path = lib.clone();
    let remote = scripted_gateway(Arc::new(move |method, params| {
        let character = params
            .pointer("/position/character")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        match method {
            "prodCode/structuralReplace" => answers::whole_file(
                &path,
                JOIN,
                &JOIN.replace("join(\"x\", \"y\")", "join(\"x\")"),
            ),
            // `a` (1:13) is used in the body; `b` (1:22) is not; `join` (1:8) is called once.
            "textDocument/references" if character == 12 => answers::locations(&path, &[(2, 5)]),
            "textDocument/references" if character == 21 => serde_json::json!([]),
            "textDocument/references" => answers::locations(&path, &[(6, 5)]),
            "textDocument/diagnostic" => answers::no_diagnostics(),
            _ => serde_json::Value::Null,
        }
    }))
    .await;
    let args = |character: u32| serde_json::json!({ "path": "src/lib.rs", "line": 1, "character": character });

    let refused = prod_code_mcp::tools::execute_tool(remote, &root, "code_safe_delete", args(13))
        .await
        .expect_err("`a` is still used by the body");
    assert!(
        format!("{refused:#}").contains("still used by the body"),
        "{refused:#}"
    );
    assert_eq!(ws.read("src/lib.rs"), JOIN);

    let done = prod_code_mcp::tools::execute_tool(remote, &root, "code_safe_delete", args(22))
        .await
        .expect("`b` is removed");
    assert!(!done.is_error, "{done:?}");
    let written = ws.read("src/lib.rs");
    assert!(
        written.contains("pub fn join(a: &str) -> String {"),
        "{written}"
    );
    assert!(written.contains("    join(\"x\")\n"), "{written}");

    // Not a parameter, and the gateway has no edit for it: an error, not "deleted".
    let nothing = scripted_gateway(Arc::new(|_method, _params| serde_json::Value::Null)).await;
    let none = prod_code_mcp::tools::execute_tool(
        nothing,
        &root,
        "code_safe_delete",
        serde_json::json!({ "path": "src/lib.rs", "line": 2, "character": 5 }),
    )
    .await
    .expect("the tool answers");
    assert!(none.is_error, "{none:?}");
    assert_eq!(ws.read("src/lib.rs"), written);
}

const CLAMP: &str = "pub const LIMIT: u32 = 10;\n\npub fn clamp(x: u32, max: u32) -> u32 {\n    if x > max { max } else { x }\n}\n\npub fn a(v: u32) -> u32 {\n    clamp(v, LIMIT)\n}\n\npub fn b(v: u32) -> u32 {\n    clamp(v + 1, LIMIT)\n}\n";

/// Inlining a parameter every caller passes the same constant for: the value is bound at the top
/// of the body and the argument leaves every call. Calls that disagree, a value that may be the
/// caller's local, and the function used as a value are refused.
#[tokio::test]
async fn a_constant_every_caller_passes_moves_into_the_body() {
    let ws = workspace();
    let root = ws.root();
    let lib = write(&ws, "src/lib.rs", CLAMP);
    commit(&ws);
    let script = |refs: Vec<(u32, u32)>| {
        let l = lib.clone();
        scripted_gateway(Arc::new(move |method, _params| match method {
            "textDocument/references" => answers::locations(&l, &refs),
            "textDocument/diagnostic" => answers::no_diagnostics(),
            _ => serde_json::Value::Null,
        }))
    };

    let done = prod_code_mcp::inline_parameter::inline_parameter(
        script(vec![(8, 5), (12, 5)]).await,
        &root,
        &lib,
        3,
        22,
        true,
        false,
    )
    .await
    .expect("the parameter is inlined");
    assert_eq!(
        (
            done.parameter.as_str(),
            done.value.as_str(),
            done.rewritten_calls
        ),
        ("max", "LIMIT", 2)
    );
    assert!(done.applied);
    let written = ws.read("src/lib.rs");
    for expected in [
        "pub fn clamp(x: u32) -> u32 {\n    let max: u32 = LIMIT;\n    if x > max",
        "    clamp(v)\n",
        "    clamp(v + 1)\n",
    ] {
        assert!(written.contains(expected), "{expected}\n{written}");
    }

    // The calls disagree, or pass the caller's own local, or use the function as a value.
    for (text, why) in [
        (
            CLAMP.replace("clamp(v + 1, LIMIT)", "clamp(v + 1, 20)"),
            "do not agree on `max`",
        ),
        (
            CLAMP.replace("LIMIT)", "limit)"),
            "may name something of the caller's",
        ),
    ] {
        std::fs::write(&lib, &text).unwrap();
        let err = prod_code_mcp::inline_parameter::inline_parameter(
            script(vec![(8, 5), (12, 5)]).await,
            &root,
            &lib,
            3,
            22,
            true,
            false,
        )
        .await
        .expect_err("refused");
        assert!(format!("{err:#}").contains(why), "{err:#}");
        assert_eq!(ws.read("src/lib.rs"), text, "nothing was written");
    }
    let with_value = format!("{CLAMP}\npub fn value() -> fn(u32, u32) -> u32 {{\n    clamp\n}}\n");
    std::fs::write(&lib, &with_value).unwrap();
    let err = prod_code_mcp::inline_parameter::inline_parameter(
        script(vec![(8, 5), (12, 5), (16, 5)]).await,
        &root,
        &lib,
        3,
        22,
        true,
        false,
    )
    .await
    .expect_err("the function used as a value blocks the write");
    assert!(format!("{err:#}").contains("used as a value"), "{err:#}");
    assert_eq!(ws.read("src/lib.rs"), with_value);
}

const TOTAL_FN: &str =
    "pub mod util;\n\npub fn total(xs: &[u32]) -> u32 {\n    xs.iter().sum()\n}\n";
const TOTAL_CALLER: &str = "pub fn report(xs: &[u32]) -> String {\n    let t: u32 = crate::total(xs);\n    format!(\"{t}\")\n}\n";

/// A new return type and visibility are written into the declaration in the same edit as the
/// parameters, and a caller the rewrite did not touch is type-checked against them: its error
/// stops the write.
#[tokio::test]
async fn a_return_type_and_visibility_change_checks_every_caller() {
    let ws = workspace();
    let root = ws.root();
    let lib = write(&ws, "src/lib.rs", TOTAL_FN);
    let util = write(&ws, "src/util.rs", TOTAL_CALLER);
    commit(&ws);
    let keep = [prod_code_mcp::signature::parse_param("xs").unwrap()];

    // The caller's file is clean on disk and rejected against the proposal.
    let pulls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let u = util.clone();
    let remote = scripted_gateway(Arc::new(move |method, params| match method {
        "textDocument/references" => answers::locations(&u, &[(2, 25)]),
        "textDocument/diagnostic"
            if params.to_string().contains("util.rs")
                && !pulls
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                    .is_multiple_of(2) =>
        {
            serde_json::json!({ "kind": "full", "items": [
                { "severity": 1, "code": "E0308", "message": "expected u32, found u64",
                  "range": { "start": { "line": 1, "character": 17 }, "end": { "line": 1, "character": 33 } } }
            ] })
        }
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;
    let modifiers = prod_code_mcp::signature::Modifiers {
        returns: Some("u64".into()),
        visibility: Some("pub(crate)".into()),
    };
    let change = prod_code_mcp::signature::change_with(
        remote, &root, &lib, 3, 8, &keep, &modifiers, false, false,
    )
    .await
    .expect("the dry run reports");
    assert_eq!(change.returns, Some(("u32".into(), "u64".into())));
    assert_eq!(change.visibility, Some(("pub".into(), "pub(crate)".into())));
    let text = &change.rewritten.first().expect("the declaration's file").1;
    assert!(
        text.contains("pub(crate) fn total(xs: &[u32]) -> u64 {"),
        "{text}"
    );
    assert!(
        change
            .diagnostics
            .iter()
            .any(|d| d.contains("expected u32, found u64") && d.contains("src/util.rs")),
        "the caller is checked: {:?}",
        change.diagnostics
    );
    let report = change.render(10_000);
    assert!(report.contains("- returns: `u32` → `u64`"), "{report}");
    assert!(
        report.contains("- visibility: `pub` → `pub(crate)`"),
        "{report}"
    );
    assert_eq!(ws.read("src/lib.rs"), TOTAL_FN, "a dry run writes nothing");
}

const CONN: &str = "pub struct Conn {\n    timeout: u64,\n}\n\nimpl Conn {\n    pub fn timeout(&self) -> u64 {\n        self.timeout\n    }\n\n    pub fn set_timeout(&mut self, t: u64) {\n        self.timeout = t;\n    }\n}\n\npub fn double(c: &mut Conn) {\n    c.set_timeout(c.timeout() * 2);\n}\n";

/// Renaming a field with `accessors`: the field's rename and each accessor's, every one computed
/// by the analyzer against the checkout, are merged into one change, even where two of them land
/// on one line; two renames that would change the same text are refused.
#[tokio::test]
async fn a_field_is_renamed_with_its_accessors_in_one_change() {
    let ws = workspace();
    let root = ws.root();
    let lib = write(&ws, "src/lib.rs", CONN);
    commit(&ws);
    let script = |getter_also_touches_the_field: bool| {
        let l = lib.clone();
        scripted_gateway(Arc::new(move |method, params| {
            let line = params
                .pointer("/position/line")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let method_hit = |name: &str, line: u32| {
                serde_json::json!([{ "name": name, "kind": 6, "containerName": "Conn",
                    "location": { "uri": format!("file://{}", l.display()),
                        "range": { "start": { "line": line - 1, "character": 11 },
                                   "end": { "line": line - 1, "character": 11 + name.len() } } } }])
            };
            match method {
                "textDocument/definition" => serde_json::json!([{
                    "uri": format!("file://{}", l.display()),
                    "range": { "start": { "line": 1, "character": 4 }, "end": { "line": 1, "character": 11 } }
                }]),
                "workspace/symbol" => match params.get("query").and_then(|q| q.as_str()) {
                    Some("timeout") => method_hit("timeout", 6),
                    Some("set_timeout") => method_hit("set_timeout", 10),
                    _ => serde_json::json!([]),
                },
                "textDocument/rename" => {
                    let renamed = match line {
                        1 => CONN
                            .replace("    timeout: u64", "    deadline: u64")
                            .replace("self.timeout", "self.deadline"),
                        5 if getter_also_touches_the_field => CONN
                            .replace("fn timeout(", "fn deadline(")
                            .replace("self.timeout\n", "self.other\n"),
                        5 => CONN
                            .replace("fn timeout(", "fn deadline(")
                            .replace("c.timeout()", "c.deadline()"),
                        9 => CONN.replace("set_timeout", "set_deadline"),
                        _ => return serde_json::Value::Null,
                    };
                    answers::whole_file(&l, CONN, &renamed)
                }
                "textDocument/diagnostic" => answers::no_diagnostics(),
                _ => serde_json::Value::Null,
            }
        }))
    };
    let args = serde_json::json!({
        "path": "src/lib.rs", "line": 2, "character": 5, "new_name": "deadline", "accessors": true
    });

    let done =
        prod_code_mcp::tools::execute_tool(script(false).await, &root, "code_rename", args.clone())
            .await
            .expect("the rename runs");
    assert!(!done.is_error, "{done:?}");
    let written = ws.read("src/lib.rs");
    assert_eq!(
        written,
        CONN.replace("timeout", "deadline"),
        "every rename in one change"
    );

    std::fs::write(&lib, CONN).unwrap();
    let refused =
        prod_code_mcp::tools::execute_tool(script(true).await, &root, "code_rename", args)
            .await
            .expect("the tool answers");
    assert!(refused.is_error, "{refused:?}");
    let text = format!("{refused:?}");
    assert!(text.contains("touches the same text"), "{text}");
    assert_eq!(ws.read("src/lib.rs"), CONN, "nothing was written");
}

/// A move into a module that does not exist yet creates the file and declares it in its parent,
/// and the file the item left still gets the import it needs: the declaration is added after the
/// imports, whose positions it would otherwise shift. A new directory with no module file of its
/// own is refused.
#[tokio::test]
async fn a_move_into_a_new_module_creates_and_declares_it() {
    let ws = workspace();
    let root = ws.root();
    write(
        &ws,
        "Cargo.toml",
        "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    let source = "pub fn helper(x: u32) -> u32 {\n    x * 2\n}\n\npub fn run() -> u32 {\n    helper(21)\n}\n";
    let lib = write(&ws, "src/lib.rs", source);
    commit(&ws);
    let l = lib.clone();
    let remote = scripted_gateway(Arc::new(move |method, _params| match method {
        "textDocument/documentSymbol" => serde_json::json!([
            answers::document_symbol("helper", 12, 1, 3, 8),
            answers::document_symbol("run", 12, 5, 7, 8),
        ]),
        "textDocument/references" => answers::locations(&l, &[(6, 5)]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;
    let util = root.join("src/util.rs");
    let moved = prod_code_mcp::move_item::move_item(remote, &root, &lib, 1, 8, &util, true, false)
        .await
        .expect("the move runs");
    assert!(moved.applied);
    assert_eq!(
        moved.created,
        Some(("src/util.rs".to_string(), "src/lib.rs".to_string()))
    );
    assert_eq!(
        ws.read("src/util.rs"),
        "pub fn helper(x: u32) -> u32 {\n    x * 2\n}\n"
    );
    let lib_now = ws.read("src/lib.rs");
    assert!(lib_now.starts_with("pub mod util;\n"), "{lib_now}");
    assert!(lib_now.contains("use crate::util::helper;"), "{lib_now}");
    assert!(!lib_now.contains("fn helper"), "{lib_now}");

    let remote = scripted_gateway(Arc::new(|_method, _params| serde_json::Value::Null)).await;
    let err = prod_code_mcp::move_item::move_item(
        remote,
        &root,
        &lib,
        5,
        8,
        &root.join("src/nowhere/deep.rs"),
        true,
        false,
    )
    .await
    .expect_err("no module file declares `nowhere`");
    assert!(
        format!("{err:#}").contains("create the parent module first"),
        "{err:#}"
    );
}

const AREA: &str = "pub fn area(w: u32, h: u32) -> u32 {\n    let a = (w + 1) * (h + 1);\n    let b = (w + 1) * 2;\n    a + b + (w + 1)\n}\n";

/// Every occurrence of the selected expression in the function reads the new variable, bound
/// once above the first; an expression that calls, or reads a name that changes on the way
/// (in a loop that runs a later occurrence again, too), is refused, and so is a result that
/// does not compile.
#[tokio::test]
async fn a_variable_is_introduced_for_every_occurrence() {
    let ws = workspace();
    let root = ws.root();
    let lib = write(&ws, "src/lib.rs", AREA);
    commit(&ws);
    let clean = || {
        scripted_gateway(Arc::new(move |method, _params| match method {
            "textDocument/diagnostic" => answers::no_diagnostics(),
            _ => serde_json::Value::Null,
        }))
    };
    let introduce = |remote, name: &'static str| {
        let (root, lib) = (root.clone(), lib.clone());
        async move {
            prod_code_mcp::introduce_variable::introduce_variable(
                remote,
                &root,
                &lib,
                (2, 13),
                (2, 20),
                name,
                true,
                false,
            )
            .await
        }
    };

    let done = introduce(clean().await, "w1").await.expect("introduced");
    assert_eq!((done.expression.as_str(), done.occurrences), ("w + 1", 3));
    assert!(done.applied);
    assert_eq!(
        ws.read("src/lib.rs"),
        "pub fn area(w: u32, h: u32) -> u32 {\n    let w1 = w + 1;\n    let a = w1 * (h + 1);\n    let b = w1 * 2;\n    a + b + w1\n}\n"
    );
    assert!(done.render().contains("-    let a = (w + 1) * (h + 1);"));

    // The first occurrence is inside an `if`, the last after it: the binding goes above the
    // statement that holds the `if`, where both can see it.
    let nested = "pub fn area(w: u32, h: u32) -> u32 {\n    let a = if h > 2 {\n        (w + 1) * h\n    } else {\n        0\n    };\n    a + (w + 1)\n}\n";
    std::fs::write(&lib, nested).unwrap();
    prod_code_mcp::introduce_variable::introduce_variable(
        clean().await,
        &root,
        &lib,
        (3, 9),
        (3, 16),
        "w1",
        true,
        false,
    )
    .await
    .expect("introduced above the `if`");
    assert_eq!(
        ws.read("src/lib.rs"),
        "pub fn area(w: u32, h: u32) -> u32 {\n    let w1 = w + 1;\n    let a = if h > 2 {\n        w1 * h\n    } else {\n        0\n    };\n    a + w1\n}\n"
    );

    for (text, why) in [
        (
            AREA.replace("(w + 1)", "(w.pow(2))"),
            "cannot be evaluated once",
        ),
        (
            AREA.replace("    let b", "    let w = 3;\n    let b"),
            "`w` changes",
        ),
        (
            "pub fn area(mut w: u32, h: u32) -> u32 {\n    let a = (w + 1) * (h + 1);\n    while w < h {\n        let _ = (w + 1);\n        w += 2;\n    }\n    a\n}\n"
                .to_string(),
            "`w` changes",
        ),
    ] {
        std::fs::write(&lib, &text).unwrap();
        let err = introduce(clean().await, "w1")
            .await
            .expect_err("refused");
        assert!(format!("{err:#}").contains(why), "{err:#}");
        assert_eq!(ws.read("src/lib.rs"), text, "nothing was written");
    }

    // The analyzer's error in the proposal stops the write.
    std::fs::write(&lib, AREA).unwrap();
    let pulls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let rejecting = scripted_gateway(Arc::new(move |method, _params| match method {
        "textDocument/diagnostic"
            if pulls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                .is_multiple_of(2) =>
        {
            answers::no_diagnostics()
        }
        "textDocument/diagnostic" => serde_json::json!({ "kind": "full", "items": [
            { "severity": 1, "code": "E0425", "message": "cannot find value `w1` in this scope",
              "range": { "start": { "line": 4, "character": 12 }, "end": { "line": 4, "character": 14 } } }
        ] }),
        _ => serde_json::Value::Null,
    }))
    .await;
    let err = introduce(rejecting, "w1")
        .await
        .expect_err("a result that does not compile is not written");
    assert!(format!("{err:#}").contains("does not compile"), "{err:#}");
    assert_eq!(ws.read("src/lib.rs"), AREA);
    assert!(introduce(clean().await, "w 1").await.is_err());
}
