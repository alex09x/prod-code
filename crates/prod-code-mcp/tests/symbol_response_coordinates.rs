//! Malformed LSP response coordinates are evidence failures, never wrapped positions that a
//! symbol-addressable tool can act on. Healthy servers always send numbers here, so scripted
//! gateways cover the protocol boundary directly.

use prod_code_mcp::tools::{execute_tool, resolve_symbol, workspace_symbol_search};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

fn rust_workspace(source: &str) -> Workspace {
    Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        ("src/lib.rs", source),
    ])
}

fn symbol(name: &str, path: &std::path::Path, start: Value) -> Value {
    json!({
        "name": name,
        "kind": 12,
        "location": {
            "uri": answers::uri(path),
            "range": { "start": start }
        }
    })
}

fn methods() -> (Arc<Mutex<Vec<String>>>, Arc<Mutex<Vec<String>>>) {
    let methods = Arc::new(Mutex::new(Vec::new()));
    (Arc::clone(&methods), methods)
}

fn tree(root: &std::path::Path) -> std::collections::BTreeMap<String, Option<Vec<u8>>> {
    fn visit(
        root: &std::path::Path,
        path: &std::path::Path,
        files: &mut std::collections::BTreeMap<String, Option<Vec<u8>>>,
    ) {
        for entry in std::fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            if entry.file_name() == ".git" {
                continue;
            }
            let path = entry.path();
            let rel = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            if entry.file_type().unwrap().is_dir() {
                files.insert(rel, None);
                visit(root, &path, files);
            } else {
                files.insert(rel, Some(std::fs::read(&path).unwrap()));
            }
        }
    }
    let mut files = std::collections::BTreeMap::new();
    visit(root, root, &mut files);
    files
}

/// Every malformed required coordinate is refused before `code_migrate_type` can send a
/// refactoring request, including with both write knobs enabled. This was a success on the base
/// for values whose `as u32 + 1` wrapped in release builds.
#[tokio::test]
async fn malformed_workspace_coordinates_refuse_before_forced_apply() {
    let bad_starts = vec![
        ("missing line", json!({ "character": 0 })),
        ("missing character", json!({ "line": 0 })),
        ("null", json!({ "line": Value::Null, "character": 0 })),
        ("string", json!({ "line": "0", "character": 0 })),
        ("fraction", json!({ "line": 0.5, "character": 0 })),
        ("negative", json!({ "line": -1, "character": 0 })),
        (
            "too wide",
            json!({ "line": 4_294_967_296u64, "character": 0 }),
        ),
        (
            "overflow increment",
            json!({ "line": u32::MAX, "character": 0 }),
        ),
        (
            "character too wide",
            json!({ "line": 0, "character": 4_294_967_296u64 }),
        ),
        (
            "character overflow increment",
            json!({ "line": 0, "character": u32::MAX }),
        ),
    ];
    for (label, start) in bad_starts {
        let ws = rust_workspace("pub fn target() {}\n");
        let path = ws.path("src/lib.rs");
        let (record, seen) = methods();
        let gateway = ScriptedGateway::start(move |method, _| {
            record.lock().unwrap().push(method.to_string());
            match method {
                "prod-code/handshake" => json!({ "index_gated": true }),
                "workspace/symbol" => json!([symbol("target", &path, start.clone())]),
                _ => Value::Null,
            }
        })
        .await;
        let before = tree(&ws.root());
        let error = execute_tool(
            gateway.addr(),
            &ws.root(),
            "code_migrate_type",
            json!({ "symbol": "target", "to": "u64", "apply": true, "force": true }),
        )
        .await
        .expect_err(label);
        assert!(
            format!("{error:#}").contains("malformed LSP"),
            "{label}: {error:#}"
        );
        assert_eq!(tree(&ws.root()), before, "{label} changed the checkout");
        let seen = seen.lock().unwrap();
        assert_eq!(gateway.calls(), 1, "{label}: {seen:?}");
        assert!(
            !seen.iter().any(|method| {
                matches!(
                    method.as_str(),
                    "textDocument/rename"
                        | "prodCode/structuralReplace"
                        | "textDocument/diagnostic"
                )
            }),
            "{label}: a refactoring request followed malformed evidence: {seen:?}"
        );
    }
}

#[tokio::test]
async fn malformed_matching_hit_cannot_make_another_hit_uniquely_actionable() {
    for malformed_first in [false, true] {
        let ws = rust_workspace("pub fn target() {}\n");
        let path = ws.path("src/lib.rs");
        let outside_candidate = ws.path("src/other.rs");
        let good = symbol("target", &path, json!({ "line": 0, "character": 7 }));
        let bad = symbol(
            "target",
            &outside_candidate,
            json!({ "line": 4_294_967_296u64, "character": 7 }),
        );
        let hits = if malformed_first {
            json!([bad, good])
        } else {
            json!([good, bad])
        };
        let (record, seen) = methods();
        let gateway = ScriptedGateway::start(move |method, _| {
            record.lock().unwrap().push(method.to_string());
            match method {
                "prod-code/handshake" => json!({ "index_gated": true }),
                "workspace/symbol" => hits.clone(),
                _ => Value::Null,
            }
        })
        .await;
        let before = tree(&ws.root());
        let error = execute_tool(
            gateway.addr(),
            &ws.root(),
            "code_migrate_type",
            json!({ "symbol": "target", "to": "u64", "apply": true, "force": true }),
        )
        .await
        .expect_err("malformed competing evidence must refuse before choosing a candidate");
        assert!(format!("{error:#}").contains("malformed LSP"), "{error:#}");
        assert_eq!(gateway.calls(), 1, "{:?}", seen.lock().unwrap());
        assert_eq!(tree(&ws.root()), before);
    }
}

/// Both shapes document symbols legitimately use preserve ordinary zero-based LSP positions,
/// but a matching member with malformed evidence must abort lookup rather than be dropped.
#[tokio::test]
async fn flat_and_nested_members_propagate_malformed_coordinates() {
    for nested in [false, true] {
        let ws = rust_workspace("pub struct Thing;\nimpl Thing { pub fn change() {} }\n");
        let path = ws.path("src/lib.rs");
        let type_hit = answers::symbol("Thing", 23, &path, 1, 12);
        let mut member = json!({
            "name": "change",
            "kind": 6,
            "selectionRange": { "start": { "line": u32::MAX, "character": 0 } }
        });
        let outline = if nested {
            json!([answers::nested(
                answers::document_symbol("Thing", 23, 1, 2, 12),
                vec![member],
            )])
        } else {
            member["containerName"] = json!("Thing");
            json!([answers::document_symbol("Thing", 23, 1, 2, 12), member])
        };
        let (record, seen) = methods();
        let gateway = ScriptedGateway::start(move |method, params| {
            record.lock().unwrap().push(method.to_string());
            match method {
                "prod-code/handshake" => json!({ "index_gated": true }),
                "workspace/symbol" => match params.get("query").and_then(Value::as_str) {
                    Some("Thing") => json!([type_hit.clone()]),
                    _ => json!([]),
                },
                "textDocument/documentSymbol" => outline.clone(),
                _ => Value::Null,
            }
        })
        .await;
        let error = resolve_symbol(gateway.addr(), &ws.root(), "Thing::change", None)
            .await
            .expect_err(if nested {
                "nested member"
            } else {
                "flat member"
            });
        assert!(format!("{error:#}").contains("malformed LSP member symbol `change`"));
        assert_eq!(gateway.calls(), 3, "{:?}", seen.lock().unwrap());
    }
}

/// An unindexed declaration is recovered from a nested language project's outline. Its matching
/// malformed response must be returned as an error, not converted to an empty search and a
/// misleading not-found answer.
#[tokio::test]
async fn fallback_named_declaration_propagates_malformed_coordinates() {
    let ws = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        ("src/lib.rs", "pub fn root() {}\n"),
        ("nested/other.go", "package nested\nfunc target() {}\n"),
    ]);
    let (record, seen) = methods();
    let gateway = ScriptedGateway::start(move |method, _| {
        record.lock().unwrap().push(method.to_string());
        match method {
            "prod-code/handshake" => json!({ "index_gated": true }),
            "workspace/symbol" => json!([]),
            "textDocument/documentSymbol" => json!([{
                "name": "target", "kind": 12,
                "selectionRange": { "start": { "line": 0, "character": "bad" } }
            }]),
            _ => Value::Null,
        }
    })
    .await;
    let error = resolve_symbol(gateway.addr(), &ws.root(), "target", None)
        .await
        .expect_err("malformed fallback declaration");
    assert!(format!("{error:#}").contains("malformed LSP named declaration `target`"));
    assert!(
        seen.lock()
            .unwrap()
            .iter()
            .any(|method| method == "textDocument/documentSymbol"),
        "the fallback outline was not reached"
    );
}

/// Zero is a valid LSP coordinate, and UTF-16 character counts remain unchanged when decoding a
/// healthy response.
#[tokio::test]
async fn valid_zero_based_and_utf16_workspace_coordinates_are_preserved() {
    let ws = rust_workspace("//😀target\npub fn other() {}\n");
    let path = ws.path("src/lib.rs");
    let gateway = ScriptedGateway::start(move |method, _| match method {
        "prod-code/handshake" => json!({ "index_gated": true }),
        "workspace/symbol" => json!([
            symbol("zero", &path, json!({ "line": 0, "character": 0 })),
            symbol("target", &path, json!({ "line": 0, "character": 4 }))
        ]),
        _ => Value::Null,
    })
    .await;
    let hits = workspace_symbol_search(gateway.addr(), &ws.root(), "target", None, 10)
        .await
        .expect("valid positions decode");
    assert_eq!((hits[0].line, hits[0].col), (1, 1));
    assert_eq!(
        (hits[1].line, hits[1].col),
        (1, 5),
        "😀 is two UTF-16 units"
    );
}
