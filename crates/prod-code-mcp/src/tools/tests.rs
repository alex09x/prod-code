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

#[test]
fn name_scan_source_read_stops_at_the_per_file_budget() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("large.rs");
    let mut contents = vec![b'x'; super::MAX_NAME_SCAN_FILE_BYTES as usize + 16];
    contents.extend_from_slice(b"target_at_end");
    std::fs::write(&path, contents).unwrap();

    let scanned = super::read_name_scan_text(&path).unwrap();
    assert_eq!(scanned.len(), super::MAX_NAME_SCAN_FILE_BYTES as usize);
    assert!(!super::names_word(&scanned, "target_at_end"));
}

#[test]
fn protobuf_outline_skips_complete_option_and_reserved_statements() {
    let proto = r#"
            syntax = "proto3";
            package example;
            option deprecated = true;
            message Request {
                option deprecated = true;
                reserved 2, 4 to 6;
                extensions 100 to max;
                string name = 1;
            }
            enum State {
                option deprecated = true;
                reserved "OLD";
                READY = 0;
            }
        "#;
    let outline = super::protobuf_outline(
        proto,
        "api.proto",
        &super::OutlineOptions::all(10, false, ""),
    );
    assert!(outline.contains("[Field] name"), "{outline}");
    assert!(outline.contains("[EnumMember] READY"), "{outline}");
    assert!(!outline.contains("deprecated"), "{outline}");
    assert!(!outline.contains("extensions"), "{outline}");
    assert!(!outline.contains("reserved"), "{outline}");
}

/// A declaration is the name right after a declaring keyword or a Go receiver; a use, a
/// path or a local binding is not (#379).
#[test]
fn a_declaration_is_the_name_after_a_declaring_keyword() {
    let at = super::declared_at;
    assert_eq!(at("pub struct Lost {", "Lost"), Some(11));
    assert_eq!(at("pub(crate) fn go(x: u8) {}", "go"), Some(14));
    assert_eq!(at("func (s *Server) Serve() error {", "Serve"), Some(17));
    assert_eq!(at("func Serve() {", "Serve"), Some(5));
    assert_eq!(at("    async def fetch(self):", "fetch"), Some(14));
    assert_eq!(at("export function render() {", "render"), Some(16));
    assert_eq!(at("macro_rules! twice {", "twice"), Some(13));
    assert_eq!(at("use crate::Lost;", "Lost"), None);
    assert_eq!(at("    let lost = Lost::new();", "Lost"), None);
    assert_eq!(at("impl Display for Lost {", "Lost"), None);
    assert_eq!(at("pub struct Lostness;", "Lost"), None);
    assert_eq!(
        at("    pub normalized_orders: u64,", "normalized_orders"),
        Some(8)
    );
    assert_eq!(at("    pub(crate) count: usize,", "count"), Some(15));
    assert_eq!(at("    name: String,", "name"), Some(4));
}

#[test]
fn an_item_ends_where_its_brackets_or_its_indentation_do() {
    let rust = [
        "/// Adds.",
        "#[inline]",
        "fn add(a: u8) -> u8 {",
        "    let open = '{'; // a { in a comment",
        "    let s = \"}}\";",
        "    a",
        "}",
        "fn next() {}",
    ];
    assert_eq!(super::item_end(&rust, 2), 6);
    assert_eq!(super::with_leading_docs(&rust, 2), 0);
    assert_eq!(super::item_end(&rust, 7), 7);
    let python = ["def f(x):", "    y = x", "", "    return y", "z = 1"];
    assert_eq!(super::item_end(&python, 0), 3);
    assert_eq!(super::item_end(&["type A = B;", "fn c() {}"], 0), 0);
    let go = ["type T struct {", "\tA int", "\tB string", "}"];
    assert_eq!(super::item_end(&go, 0), 3);
}

#[test]
fn a_body_is_numbered_and_capped() {
    let lines: Vec<String> = (1..=400).map(|n| format!("line {n}")).collect();
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let text = super::numbered_lines(&refs, 9, 12);
    assert_eq!(
        text,
        "10 | line 10\n11 | line 11\n12 | line 12\n13 | line 13"
    );
    // Numbers are right-aligned to the widest.
    assert!(super::numbered_lines(&refs, 7, 10).starts_with(" 8 | line 8"));
    let long = super::numbered_lines(&refs, 0, 399);
    assert!(
        long.ends_with("… 100 more line(s)"),
        "{}",
        &long[long.len() - 40..]
    );
    assert_eq!(long.lines().count(), super::MAX_BODY_LINES + 1);
}

#[test]
fn the_innermost_outline_range_holding_a_position_wins() {
    let outline = serde_json::json!([{
        "name": "Store",
        "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 20, "character": 1 } },
        "children": [{
            "name": "sum",
            "range": { "start": { "line": 4, "character": 4 }, "end": { "line": 8, "character": 5 } }
        }]
    }, {
        "name": "flat",
        "location": { "range": { "start": { "line": 30, "character": 0 }, "end": { "line": 33, "character": 1 } } }
    }]);
    let mut best = None;
    super::innermost_holding(&outline, (4, 11), &mut best);
    assert_eq!(best, Some(((4, 4), (8, 5))));
    let mut flat = None;
    super::innermost_holding(&outline, (30, 3), &mut flat);
    assert_eq!(flat, Some(((30, 0), (33, 1))));
}

#[test]
fn a_workspace_query_is_anchored_in_the_root_project_not_a_crate_it_leaves_out() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let write = |rel: &str, text: &str| {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    write(
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/core\"]\nresolver = \"2\"\n",
    );
    write(
        "crates/core/Cargo.toml",
        "[package]\nname = \"core\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write("crates/core/src/lib.rs", "pub fn core() {}\n");
    // A crate with a workspace of its own and the shorter path: its own project (#335).
    write(
        "ext/zed/Cargo.toml",
        "[package]\nname = \"zed\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n",
    );
    write("ext/zed/src/lib.rs", "pub fn ext() {}\n");
    let anchor = super::representative_source_file(root).unwrap();
    assert!(
        anchor.ends_with("crates/core/src/lib.rs"),
        "the anchor is the root workspace's: {}",
        anchor.display()
    );
}

#[test]
fn rewritten_files_reads_document_changes_and_skips_the_rest() {
    let edit = serde_json::json!({
        "documentChanges": [
            { "kind": "rename", "oldUri": "file:///w/a.rs", "newUri": "file:///w/b.rs" },
            { "textDocument": { "uri": "file:///w/b.rs", "version": null },
              "edits": [ { "range": {}, "newText": "fn b() {}\n" } ] },
            { "textDocument": { "uri": "file:///w/c.rs" } }
        ]
    });
    assert_eq!(
        rewritten_files(&edit),
        vec![("/w/b.rs".to_string(), "fn b() {}\n".to_string())]
    );
    assert!(rewritten_files(&serde_json::json!({})).is_empty());
}

#[test]
fn codemod_schema_requires_a_rule_and_offers_apply() {
    let tool = list_tools()
        .into_iter()
        .find(|t| t.name == "code_codemod")
        .expect("code_codemod is listed");
    assert_eq!(tool.input_schema["required"], serde_json::json!(["rule"]));
    assert_eq!(tool.input_schema["properties"]["apply"]["type"], "boolean");
    assert!(
        tool.input_schema["properties"]["rule"]["description"]
            .as_str()
            .unwrap()
            .contains("==>>")
    );
}

#[test]
fn shadow_run_schema_takes_hypotheses_and_argv() {
    let tool = list_tools()
        .into_iter()
        .find(|t| t.name == "code_shadow_run")
        .expect("code_shadow_run is listed");
    let required: Vec<&str> = tool.input_schema["required"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(required, vec!["hypotheses", "argv"]);
    let item = &tool.input_schema["properties"]["hypotheses"]["items"];
    assert_eq!(item["required"], serde_json::json!(["name"]));
    assert_eq!(
        item["properties"]["edits"]["items"]["required"],
        serde_json::json!(["path", "new_text"])
    );
    assert_eq!(tool.input_schema["properties"]["apply"]["type"], "boolean");
    assert_eq!(
        tool.input_schema["properties"]["in_memory"]["type"],
        "boolean"
    );
    assert_eq!(tool.input_schema["properties"]["ram"]["type"], "boolean");
}

#[test]
fn symbol_addressable_tools_advertise_symbol_and_do_not_require_a_position() {
    for tool in list_tools() {
        let props = tool.input_schema["properties"]
            .as_object()
            .expect("schema has properties");
        let required: Vec<&str> = tool.input_schema["required"]
            .as_array()
            .map(|r| r.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();
        if SYMBOL_ADDRESSABLE.contains(&tool.name.as_str()) {
            assert!(props.contains_key("symbol"), "{} lacks `symbol`", tool.name);
            assert!(props.contains_key("path"), "{} lacks `path`", tool.name);
            for positional in ["path", "line", "character"] {
                assert!(
                    !required.contains(&positional),
                    "{} still requires `{positional}`",
                    tool.name
                );
            }
        } else if !NAMES_A_SYMBOL.contains(&tool.name.as_str()) {
            assert!(
                !props.contains_key("symbol"),
                "{} unexpectedly takes `symbol`",
                tool.name
            );
        }
    }
    let rename = list_tools()
        .into_iter()
        .find(|t| t.name == "code_rename")
        .unwrap();
    let required: Vec<&str> = rename.input_schema["required"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(required, vec!["new_name"]);
}

#[test]
fn validate_edits_schema_accepts_alternative_inputs() {
    let tool = list_tools()
        .into_iter()
        .find(|t| t.name == "code_validate_edits")
        .unwrap();
    // Each public input form stands alone: a diff or WorkspaceEdit must not need
    // an unrelated, dummy `edits` list to satisfy MCP client validation.
    assert!(tool.input_schema.get("required").is_none());
    assert_eq!(
        tool.input_schema["anyOf"],
        serde_json::json!([
            { "required": ["edits"] },
            { "required": ["diff"] },
            { "required": ["workspace_edit"] }
        ])
    );
    assert_eq!(
        tool.input_schema["properties"]["edits"]["items"]["required"],
        serde_json::json!(["path", "new_text"])
    );
}

#[test]
fn outline_schema_offers_locals_toggle() {
    let outline = list_tools()
        .into_iter()
        .find(|t| t.name == "code_outline")
        .unwrap();
    assert!(outline.input_schema["properties"]["include_locals"].is_object());
}

#[test]
fn qualifier_matches_resolves_go_receiver_and_package_methods() {
    let root = Path::new("/workspace");
    let hit = SymbolHit {
        path: root.join("internal/web/delegation.go"),
        name: "(*Server).verifyRenewablePrimaryTokenRecord".to_string(),
        kind: "Method",
        container: Some("prod/internal/web".to_string()),
        line: 42,
        col: 1,
    };

    // Exact receiver name returned by symbol discovery (#752)
    assert!(super::qualifier_matches(root, &hit, &["Server"]));
    // Pointer-receiver syntax
    assert!(super::qualifier_matches(root, &hit, &["(*Server)"]));
    // Package-qualified receiver
    assert!(super::qualifier_matches(root, &hit, &["web", "Server"]));
    // Fully-qualified module path
    assert!(super::qualifier_matches(
        root,
        &hit,
        &["prod", "internal", "web", "Server"]
    ));

    // Unrelated receiver must not match
    assert!(!super::qualifier_matches(root, &hit, &["Client"]));
    assert!(!super::qualifier_matches(
        root,
        &hit,
        &["otherpkg", "Server"]
    ));

    // Value receiver
    let val_hit = SymbolHit {
        path: root.join("internal/runner/runner.go"),
        name: "(Runner).Run".to_string(),
        kind: "Method",
        container: Some("runner".to_string()),
        line: 10,
        col: 1,
    };
    assert!(super::qualifier_matches(root, &val_hit, &["Runner"]));
    assert!(super::qualifier_matches(
        root,
        &val_hit,
        &["runner", "Runner"]
    ));
    assert!(!super::qualifier_matches(root, &val_hit, &["Server"]));
}

#[test]
fn declared_at_matches_var_let_val_declarations() {
    assert_eq!(
        super::declared_at("    var searchState: State", "searchState"),
        Some(8)
    );
    assert_eq!(
        super::declared_at("@Published var searchState: State", "searchState"),
        Some(15)
    );
    assert_eq!(super::declared_at("let count = 42;", "count"), Some(4));
    assert_eq!(super::declared_at("val items = listOf()", "items"), Some(4));
}
