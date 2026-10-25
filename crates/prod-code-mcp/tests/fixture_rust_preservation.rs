/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::fixture::{Fixture, generate};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::Value;
use std::{
    fs,
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

const MANIFEST: &str =
    "[package]\nname = \"fixture_probe\"\nversion = \"0.1.0\"\nedition = \"2024\"\n";

#[path = "fixture_rust_preservation/attributes.rs"]
mod attributes;
#[path = "fixture_rust_preservation/enum_fields.rs"]
mod enum_fields;

// Source locations are scripted. Diagnostic acceptance is deliberately scripted too;
// it is NOT compiler evidence. Each result separately goes through a real, typed rustc probe.
async fn fixture(source: &str, symbol: &str, depth: u32) -> (Workspace, Fixture) {
    let ws = Workspace::new(&[("Cargo.toml", MANIFEST), ("src/lib.rs", source)]);
    let file = ws.path("src/lib.rs");
    let text = source.to_string();
    let target = symbol.to_string();
    let probes = Arc::new(Mutex::new(Vec::<String>::new()));
    let captured = Arc::clone(&probes);
    let diagnostics = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&diagnostics);
    let gw = ScriptedGateway::start(move |method, params| match method {
        "workspace/symbol" => {
            let query = params["query"].as_str().unwrap_or("");
            let found = text.lines().enumerate().find(|(_, line)| {
                line.contains(&format!("struct {query}")) || line.contains(&format!("enum {query}"))
            });
            match found {
                Some((line, declaration)) if !query.is_empty() => {
                    serde_json::json!([answers::symbol(
                        query,
                        if declaration.contains("enum ") {
                            10
                        } else {
                            23
                        },
                        &file,
                        line as u32 + 1,
                        1
                    )])
                }
                _ => serde_json::json!([]),
            }
        }
        "textDocument/documentSymbol" => serde_json::json!([]),
        "textDocument/didOpen" | "textDocument/didChange" => {
            let text = params
                .pointer("/textDocument/text")
                .or_else(|| params.pointer("/contentChanges/0/text"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if text.contains("mod prod_code_generated_fixture") {
                captured.lock().unwrap().push(text.to_string());
            }
            Value::Null
        }
        "textDocument/diagnostic" => {
            counted.fetch_add(1, Ordering::Relaxed);
            answers::no_diagnostics()
        }
        _ => Value::Null,
    })
    .await;
    let result = generate(gw.addr(), &ws.root(), &target, depth, true, None)
        .await
        .unwrap();
    assert!(result.verified, "verification must remain enabled");
    assert!(result.diagnostics.is_empty());
    assert!(diagnostics.load(Ordering::Relaxed) > 0);
    assert!(
        probes
            .lock()
            .unwrap()
            .iter()
            .any(|p| p.contains(&result.value))
    );
    assert_eq!(
        ws.read("src/lib.rs"),
        source,
        "verification must restore source"
    );
    println!(
        "{symbol} value: {}\nfallbacks: {:?}",
        result.value, result.fallbacks
    );
    (ws, result)
}

fn compiles(ws: &Workspace, source: &str, result: &Fixture, assertions: &str) {
    compiles_value(ws, source, &result.type_name, &result.value, assertions);
}

fn compiles_value(ws: &Workspace, source: &str, type_name: &str, value: &str, assertions: &str) {
    let probe =
        format!("{source}\nfn main() {{ let value: {type_name} = {value}; {assertions} }}\n");
    fs::write(ws.path("probe.rs"), probe).unwrap();
    let output = Command::new("rustc")
        .current_dir(ws.root())
        .args(["--edition=2024", "-Awarnings", "probe.rs", "-o", "probe"])
        .output()
        .expect("real rustc is required");
    println!(
        "rustc --edition=2024 -Awarnings probe.rs -o probe: {}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success(),
        "generated value must type-check: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let run = Command::new(ws.path("probe")).output().unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
}

#[tokio::test]
async fn records_over_forty_fields_are_complete() {
    let fields = (0..45)
        .map(|i| format!("    field_{i}: u32,\n"))
        .collect::<String>();
    let source = format!("struct Large {{\n{fields}}}\n");
    let (ws, result) = fixture(&source, "Large", 2).await;
    compiles(&ws, &source, &result, "assert_eq!(value.field_44, 0);");
    for i in 0..45 {
        assert!(result.value.contains(&format!("field_{i}:")));
    }
    assert!(result.fallbacks.is_empty());
}

#[tokio::test]
async fn payload_first_enum_is_a_value() {
    let source = "enum Event { Data(String), Empty }\n";
    let (ws, result) = fixture(source, "Event", 2).await;
    compiles(
        &ws,
        source,
        &result,
        "assert!(matches!(value, Event::Empty | Event::Data(_)));",
    );
    assert!(result.fallbacks.is_empty());
}

#[tokio::test]
async fn raw_enum_variant_identifiers_remain_intact() {
    for (source, assertion, expected) in [
        (
            "enum Event { Empty,\n r#type(u8),\n}\n",
            "assert!(matches!(value, Event::Empty));",
            "Event::Empty",
        ),
        (
            "enum Event { r#type(String) }\n",
            "assert!(matches!(value, Event::r#type(s) if s.is_empty()));",
            "Event::r#type(String::new())",
        ),
        (
            "enum Event { r#type }\n",
            "assert!(matches!(value, Event::r#type));",
            "Event::r#type",
        ),
    ] {
        let (ws, result) = fixture(source, "Event", 2).await;
        compiles(&ws, source, &result, assertion);
        assert_eq!(result.value, expected);
        assert!(result.fallbacks.is_empty());
    }
}

#[tokio::test]
async fn private_pub_prefix_names_remain_intact() {
    let source = "struct Keys { public_key: String, published: bool, pubsub: u32 }\n";
    let (ws, result) = fixture(source, "Keys", 2).await;
    compiles(
        &ws,
        source,
        &result,
        "assert!(value.public_key.is_empty()); assert!(!value.published); assert_eq!(value.pubsub, 0);",
    );
    assert!(result.fallbacks.is_empty());
}

#[tokio::test]
async fn payload_only_enums_preserve_tuple_and_record_shapes() {
    for (symbol, source, assertion) in [
        (
            "TupleEvent",
            "enum TupleEvent { Data(String, u32) }\n",
            "assert!(matches!(value, TupleEvent::Data(s, 0) if s.is_empty()));",
        ),
        (
            "RecordEvent",
            "enum RecordEvent {\n Data {\n public_key: String,\n count: u32,\n },\n}\n",
            "assert!(matches!(value, RecordEvent::Data { public_key, count: 0 } if public_key.is_empty()));",
        ),
        (
            "WrappedEvent",
            "use std::collections::HashMap;\npub(crate) enum WrappedEvent {\n Data(\n String,\n HashMap<String, u32>,\n ),\n}\n",
            "assert!(matches!(value, WrappedEvent::Data(s, map) if s.is_empty() && map.is_empty()));",
        ),
    ] {
        let (ws, result) = fixture(source, symbol, 2).await;
        compiles(&ws, source, &result, assertion);
        assert!(result.fallbacks.is_empty());
    }
}

#[tokio::test]
async fn public_and_restricted_visibility_tokens_are_preserved() {
    let source = "pub(crate) struct Visible {\n pub public_key: String,\n pub(crate) count: u32,\n pub(self) own: bool,\n pub(in crate) scoped: u32,\n pub(in crate::nested) nested: bool,\n pub(super) parent: u32,\n}\n";
    // The source lives in `nested` for valid pub(super) and pub(in crate::nested).
    let (ws, result) = fixture(source, "Visible", 2).await;
    let wrapped = format!(
        "mod nested {{ {source}\npub fn check() {{ let value: Visible = {}; assert!(value.public_key.is_empty()); assert_eq!(value.count + value.scoped + value.parent, 0); assert!(!value.own && !value.nested); }} }}\nfn main() {{ nested::check(); }}",
        result.value
    );
    fs::write(ws.path("probe.rs"), wrapped).unwrap();
    let output = Command::new("rustc")
        .current_dir(ws.root())
        .args(["--edition=2024", "-Awarnings", "probe.rs", "-o", "probe"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(Command::new(ws.path("probe")).status().unwrap().success());
    assert!(result.fallbacks.is_empty());
    let source = "pub(crate) struct Pair(pub(self) u32, pub String);\n";
    let (ws, result) = fixture(source, "Pair", 2).await;
    compiles(
        &ws,
        source,
        &result,
        "assert_eq!(value.0, 0); assert!(value.1.is_empty());",
    );
}

#[tokio::test]
async fn public_tuple_fields_construct_non_default_elements() {
    for visibility in ["pub", "pub(crate)", "pub (crate)"] {
        let source =
            format!("struct Inner {{ flag: bool }}\nstruct Pair({visibility} (Inner, u32));\n");
        let (ws, result) = fixture(&source, "Pair", 2).await;
        compiles(
            &ws,
            &source,
            &result,
            "assert!(!value.0.0.flag); assert_eq!(value.0.1, 0);",
        );
        assert!(result.fallbacks.is_empty());
    }
}

#[tokio::test]
async fn trailing_comments_preserve_fields_and_variants() {
    for (symbol, source, assertion) in [
        (
            "Commented",
            "struct Commented {\n a: u32, // note\n b: u32,\n}\n",
            "assert_eq!(value.a + value.b, 0);",
        ),
        (
            "CommentedEvent",
            "enum CommentedEvent {\n Data(String), // note\n Empty,\n}\n",
            "assert!(matches!(value, CommentedEvent::Empty));",
        ),
        (
            "PayloadEvent",
            "enum PayloadEvent {\n Data { a: u32, // note\n b: bool }, // note\n}\n",
            "assert!(matches!(value, PayloadEvent::Data { a: 0, b: false }));",
        ),
    ] {
        let (ws, result) = fixture(source, symbol, 2).await;
        compiles(&ws, source, &result, assertion);
        assert!(result.fallbacks.is_empty());
    }
}

#[tokio::test]
async fn depth_and_cycle_fallback_contracts_remain_bounded() {
    let source = "#[derive(Default)]\nstruct Inner { count: u32 }\nstruct Outer { inner: Inner }\n";
    let (ws, result) = fixture(source, "Outer", 0).await;
    assert_eq!(result.fallbacks, ["Inner"]);
    compiles(&ws, source, &result, "assert_eq!(value.inner.count, 0);");
    let source = "#[derive(Default)]\nstruct Node { next: Option<Box<Node>>, child: Box<Link> }\n#[derive(Default)]\nstruct Link { next: Option<Box<Node>> }\n";
    let (ws, result) = fixture(source, "Node", 2).await;
    compiles(
        &ws,
        source,
        &result,
        "assert!(value.next.is_none() && value.child.next.is_none());",
    );
    assert!(result.fallbacks.is_empty());
    // A directly recursive value has no finite constructor. Preserve the explicit
    // cycle fallback instead of recursing forever; do not execute that fallback.
    let (_, result) = fixture("struct Cycle { child: Box<Cycle> }\n", "Cycle", 2).await;
    assert_eq!(result.fallbacks, ["Cycle"]);
    assert!(result.value.contains("Box::new(Default::default())"));
}

#[tokio::test]
async fn go_mock_verification_still_declares_methods_at_file_scope() {
    use prod_code_mcp::fixture::{FixtureOptions, generate_with_options};
    let source = "package demo\n\ntype Reader interface {\n Read() string\n}\n";
    let ws = Workspace::new(&[
        ("go.mod", "module demo\n\ngo 1.22\n"),
        ("reader.go", source),
    ]);
    let file = ws.path("reader.go");
    let probes = Arc::new(Mutex::new(Vec::<String>::new()));
    let capture = Arc::clone(&probes);
    let gw = ScriptedGateway::start(move |method, params| match method {
        "workspace/symbol" => serde_json::json!([answers::symbol("Reader", 11, &file, 3, 6)]),
        "textDocument/documentSymbol" => serde_json::json!([]),
        "textDocument/didOpen" | "textDocument/didChange" => {
            if let Some(text) = params
                .pointer("/textDocument/text")
                .or_else(|| params.pointer("/contentChanges/0/text"))
                .and_then(Value::as_str)
            {
                capture.lock().unwrap().push(text.to_string());
            }
            Value::Null
        }
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await;
    let result = generate_with_options(
        gw.addr(),
        &ws.root(),
        "Reader",
        FixtureOptions {
            verify: true,
            mock: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(result.verified && result.diagnostics.is_empty());
    let probes = probes.lock().unwrap();
    let probe = probes
        .iter()
        .find(|p| p.contains("func _TestProdCodeFixtureProbe"))
        .unwrap();
    let method = probe.find("func (").expect("mock method");
    let check = probe.find("func _TestProdCodeFixtureProbe").unwrap();
    assert!(method < check);
    assert_eq!(
        probe[..method].matches('{').count(),
        probe[..method].matches('}').count()
    );
    assert!(probe.contains("var _ Reader = (*MockReader)(nil)"));
}
