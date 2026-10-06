/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;

use super::super::casing::variants;
use super::super::detect::{Schema, is_structural, kind_of, label, schema_of};
use super::super::scan::{display, edit_for, lsp_position, scan, text_rewrite_decision, walk};

#[test]
fn openapi_and_graphql_are_read_for_their_structure() {
    let yaml = Path::new("/w/api/openapi.yaml");
    let spec = "openapi: 3.0.3\ncomponents:\n  schemas:\n    Order:\n      required: [id, order_id]\n      properties:\n        order_id:\n          description: The order_id of the order\n        list:\n          - order_id # the key\n          - \"order_id\"\n      x-note: order_id, then\n";
    assert_eq!(schema_of(yaml, spec), Some(Schema::OpenApi));
    assert_eq!(schema_of(yaml, "name: order_id\n"), Some(Schema::Yaml));
    assert_eq!(label(yaml, spec), Some("openapi"));
    assert_eq!(label(yaml, "a: 1\n"), Some("yaml"));
    let json = Path::new("/w/api/openapi.json");
    let doc = "{\n  \"openapi\": \"3.1.0\",\n  \"required\": [\"order_id\"],\n  \"order_id\": {\"description\": \"the order_id\"}\n}\n";
    assert_eq!(schema_of(json, doc), Some(Schema::OpenApi));
    assert_eq!(schema_of(json, "{\"a\": 1}\n"), Some(Schema::Json));
    let variants = variants("order_id", "trade_id");
    let structural = |path: &Path, text: &str| -> Vec<(u32, bool)> {
        let schema = schema_of(path, text).unwrap();
        scan(text, &variants, path)
            .iter()
            .map(|o| (o.line, is_structural(schema, text, o)))
            .collect()
    };
    assert_eq!(
        structural(yaml, spec),
        vec![
            (5, true),
            (7, true),
            (8, false),
            (10, true),
            (11, true),
            // `x-note: order_id, then` looks like a flow list: a whole value followed by a
            // comma. Prose written that way is rewritten; the diff shows it.
            (12, true),
        ]
    );
    assert_eq!(
        structural(json, doc),
        vec![(3, true), (4, true), (4, false)]
    );

    let gql = Path::new("/w/schema.graphql");
    let sdl = "type Order {\n  \"\"\"\n  Not the orderId of a trade.\n  \"\"\"\n  orderId: ID! # orderId is the key\n  \"the orderId\" total(orderId: ID): Int\n}\n";
    assert_eq!(schema_of(gql, sdl), Some(Schema::GraphQl));
    assert_eq!(label(gql, sdl), Some("graphql"));
    assert_eq!(
        structural(gql, sdl),
        vec![(3, false), (5, true), (5, false), (6, false), (6, true)]
    );
}

#[test]
fn the_walk_skips_what_is_not_source() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for (rel, text) in [
        ("src/a.rs", "fn a() {}"),
        ("schema/b.proto", "message B {}"),
        ("target/debug/c.rs", "fn c() {}"),
        ("node_modules/d/e.ts", "export {};"),
        ("logo.png", "not text"),
    ] {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let found: Vec<String> = walk(root, 512 * 1024)
        .iter()
        .map(|p| display(root, p))
        .collect();
    assert_eq!(found, ["schema/b.proto", "src/a.rs"]);
}

#[test]
fn unsupported_prose_is_left_as_evidence_not_rewritten() {
    let variants = variants("order_id", "trade_id");
    for (path, text) in [
        ("README.md", "The order_id is documented here.\n"),
        ("deploy.sh", "echo order_id\n"),
        ("settings.toml", "note = 'order_id'\n"),
        ("evidence.env", "NOTE=order_id\n"),
        ("notes.txt", "The order_id is plain prose.\n"),
    ] {
        let path = Path::new(path);
        let hits = scan(text, &variants, path);
        assert!(!hits.is_empty(), "{path:?} is retained as evidence");
        let edits: Vec<_> = hits
            .iter()
            .filter(|o| text_rewrite_decision(kind_of(path), schema_of(path, text), text, o).0)
            .map(|o| edit_for(o, &variants[o.variant]))
            .collect();
        assert!(edits.is_empty());
        let after = crate::refactor::apply_scalar_text_edits(text, &edits).unwrap();
        assert_eq!(after, text, "{path:?} is unchanged");
    }
}

#[test]
fn semantic_rename_position_uses_utf16_after_non_bmp_text() {
    let variants = variants("order_id", "trade_id");
    let text = "let label = \"😀\"; let order_id = 1;\n";
    let occurrence = scan(text, &variants, Path::new("src/lib.rs"))
        .into_iter()
        .find(|o| variants[o.variant].from == "order_id")
        .expect("the code identifier");

    // `order_id` is scalar column 22; the emoji before it occupies two UTF-16 units, so
    // its 0-based LSP character is 22 rather than the scalar-based 21.
    assert_eq!(occurrence.col, 22);
    assert_eq!(lsp_position(text, &occurrence), (0, 22));
}
