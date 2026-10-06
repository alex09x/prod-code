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
use prod_code_testkit::{ScriptedGateway, Workspace, answers};

#[test]
fn snake_case_splits_humps() {
    assert_eq!(snake_case("SliceReport"), "slice_report");
    assert_eq!(snake_case("Config"), "config");
    assert_eq!(snake_case("HTTPClient"), "h_t_t_p_client");
}

#[test]
fn declaration_is_taken_from_the_hover_block_that_declares_it() {
    let hover = "```rust\nprod_code_mcp::slice\n```\n\n```rust\npub struct SliceReport {\n    pub seed: String,\n}\n```\n\n---\n\ndocs";
    let decl = declaration_from_hover(hover).expect("a declaration");
    assert!(decl.starts_with("pub struct SliceReport"));
    assert!(declaration_from_hover("```rust\njust::a::path\n```").is_none());
}

#[test]
fn shapes_are_read_from_declarations() {
    let record =
        parse_shape("pub struct A {\n    pub a: String,\n    b: HashMap<String, u64>,\n}").unwrap();
    assert_eq!(
        record,
        Shape::Record(vec![
            ("a".into(), "String".into()),
            ("b".into(), "HashMap<String, u64>".into())
        ])
    );
    assert_eq!(
        parse_shape("pub struct B(pub u32, String);").unwrap(),
        Shape::Tuple(vec!["u32".into(), "String".into()])
    );
    assert_eq!(parse_shape("struct C;").unwrap(), Shape::Unit);
    assert_eq!(
        parse_shape("pub enum D {\n    First,\n    Second(u8),\n}").unwrap(),
        Shape::Enum(vec!["First".into(), "Second".into()])
    );
}

#[test]
fn a_doc_comment_between_fields_is_not_a_field() {
    let shape = parse_shape(
        "pub struct A {\n    /// how many\n    pub count: usize,\n    #[serde(default)]\n    pub name: String,\n}",
    )
    .unwrap();
    assert_eq!(
        shape,
        Shape::Record(vec![
            ("count".into(), "usize".into()),
            ("name".into(), "String".into())
        ])
    );
}

#[test]
fn known_types_get_a_value_and_unknown_ones_do_not() {
    assert_eq!(known_value("bool").as_deref(), Some("false"));
    assert_eq!(known_value("usize").as_deref(), Some("0"));
    assert_eq!(known_value("String").as_deref(), Some("String::new()"));
    assert_eq!(known_value("&str").as_deref(), Some("\"\""));
    assert_eq!(known_value("&'a str").as_deref(), Some("\"\""));
    assert_eq!(known_value("&mut String").as_deref(), Some("String::new()"));
    // a caller may have stripped the reference already, leaving the lifetime
    assert_eq!(known_value("'static str").as_deref(), Some("\"\""));
    assert_eq!(known_value("Option<Whatever>").as_deref(), Some("None"));
    assert_eq!(known_value("Vec<Whatever>").as_deref(), Some("Vec::new()"));
    assert_eq!(
        known_value("std::collections::HashMap<String, u64>").as_deref(),
        Some("HashMap::new()")
    );
    assert_eq!(known_value("MyStruct"), None);
}

#[test]
fn generic_arguments_survive_the_comma_split() {
    assert_eq!(
        split_top_level("a: HashMap<String, u64>, b: (u8, u8)")
            .into_iter()
            .map(|s| s.trim().to_string())
            .collect::<Vec<_>>(),
        vec!["a: HashMap<String, u64>", "b: (u8, u8)"]
    );
    assert_eq!(inner_of("Arc<Mutex<Engine>>", "Arc"), Some("Mutex<Engine>"));
    assert_eq!(inner_of("Vec<u8>", "Arc"), None);
}

#[test]
fn a_wrapper_builds_its_inner_value() {
    assert_eq!(
        wrapper_value("Arc<String>", |inner| {
            assert_eq!(inner, "String");
            "String::new()".to_string()
        })
        .as_deref(),
        Some("std::sync::Arc::new(String::new())")
    );
    assert!(wrapper_value("Vec<String>", |_| String::new()).is_none());
}

#[test]
fn the_report_says_whether_the_analyzer_accepted_it() {
    let mut f = Fixture {
        type_name: "Config".into(),
        value: "Config {\n    port: 0,\n}".into(),
        file: "src/config.rs".into(),
        fallbacks: vec!["Engine".into(), "Engine".into()],
        diagnostics: Vec::new(),
        verified: true,
        language: "rust",
        is_mock: false,
        snippet: "let config = Config {\n    port: 0,\n};".into(),
    };
    let text = f.render();
    assert!(text.contains("let config = Config {"), "{text}");
    assert!(text.contains("stands in for: Engine ("), "{text}");
    assert!(text.contains("the analyzer accepts it: 0 errors"), "{text}");
    f.diagnostics = vec!["error: missing field `host`".into()];
    assert!(f.render().contains("the analyzer rejects it"));
    f.verified = false;
    assert!(f.render().contains("not verified"));
}

#[test]
fn go_mock_methods_are_declared_at_file_scope_before_the_probe() {
    let snippet =
        "type MockReader struct{}\nfunc (m *MockReader) Read() string { return \"read\" }";
    let probe = go_mock_verification_probe("package example\n", snippet, "Reader", true);
    let probe_fn = probe.find("func _TestProdCodeFixtureProbe()").unwrap();
    let method = probe.find("func (m *MockReader) Read()").unwrap();
    assert!(method < probe_fn, "{probe}");
    assert!(probe[probe_fn..].contains("_ = &MockReader{}"), "{probe}");
    assert!(
        !probe[probe_fn..].contains("func (m *MockReader) Read()"),
        "{probe}"
    );
}

#[tokio::test]
async fn root_filenames_do_not_hide_distinct_declarations() {
    for root_name in ["src/lib.rs", "src/main.rs", "src/nested/mod.rs"] {
        let ws = Workspace::new(&[
            (root_name, "pub struct Thing { pub root: u8 }\n"),
            ("src/other.rs", "pub struct Thing { pub other: bool }\n"),
        ]);
        let first = ws.path(root_name);
        let second = ws.path("src/other.rs");
        let (a, b) = (first.clone(), second.clone());
        let gateway = ScriptedGateway::start(move |method, _| match method {
            "workspace/symbol" => serde_json::json!([
                answers::symbol("Thing", 23, &a, 1, 12),
                answers::symbol("Thing", 23, &b, 1, 12),
            ]),
            "textDocument/documentSymbol" => {
                serde_json::json!([answers::document_symbol("Thing", 23, 1, 1, 12),])
            }
            _ => serde_json::Value::Null,
        })
        .await;
        let err = generate(gateway.addr(), &ws.root(), "Thing", 1, false, None)
            .await
            .expect_err("two real declarations need a hint");
        let why = format!("{err:#}");
        assert!(
            why.contains(root_name) && why.contains("src/other.rs"),
            "{why}"
        );
        for (path, field) in [(&first, "root: 0"), (&second, "other: false")] {
            let generated = generate(gateway.addr(), &ws.root(), "Thing", 1, false, Some(path))
                .await
                .unwrap();
            assert!(generated.value.contains(field), "{}", generated.value);
            assert_eq!(
                generated.file,
                path.strip_prefix(ws.root()).unwrap().to_string_lossy()
            );
        }
    }
}

#[tokio::test]
async fn only_identical_locations_can_be_deduplicated() {
    let ws = Workspace::new(&[(
        "src/lib.rs",
        "mod a { pub struct Thing; }\nmod b { pub struct Thing; }\n",
    )]);
    let path = ws.path("src/lib.rs");
    for second_line in [1, 2] {
        let file = path.clone();
        let gateway = ScriptedGateway::start(move |method, _| match method {
            "workspace/symbol" => serde_json::json!([
                answers::symbol("Thing", 23, &file, 1, 20),
                answers::symbol("Thing", 23, &file, second_line, 20),
            ]),
            _ => serde_json::Value::Null,
        })
        .await;
        let result = resolve_type(gateway.addr(), &ws.root(), "Thing", Some(&path)).await;
        assert_eq!(result.is_ok(), second_line == 1, "{result:?}");
    }
}

#[tokio::test]
async fn a_separately_indexed_reexport_requires_the_declaring_file() {
    let ws = Workspace::new(&[
        ("src/lib.rs", "pub use other::Thing;\n"),
        ("src/other.rs", "pub struct Thing;\n"),
    ]);
    let (lib, module) = (ws.path("src/lib.rs"), ws.path("src/other.rs"));
    let m = module.clone();
    let gateway = ScriptedGateway::start(move |method, _| match method {
        "workspace/symbol" => serde_json::json!([
            answers::symbol("Thing", 23, &lib, 1, 16),
            answers::symbol("Thing", 23, &m, 1, 12),
        ]),
        _ => serde_json::Value::Null,
    })
    .await;
    assert!(
        resolve_type(gateway.addr(), &ws.root(), "Thing", None)
            .await
            .is_err()
    );
    let hit = resolve_type(gateway.addr(), &ws.root(), "Thing", Some(&module))
        .await
        .unwrap();
    assert_eq!(hit.path, module);
}
