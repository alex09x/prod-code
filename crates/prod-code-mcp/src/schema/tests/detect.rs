/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::{Path, PathBuf};

use super::super::detect::{
    Kind, Schema, in_block_string, is_mapping_key, is_structural, kind_of, label, schema_of,
    text_comment_before,
};
use super::super::types::Occurrence;

fn occ(line: u32, col: u32, len: usize) -> Occurrence {
    Occurrence {
        file: PathBuf::from("f"),
        line,
        col,
        len,
        variant: 0,
        in_string: false,
    }
}

#[test]
fn every_extension_maps_to_its_kind_case_insensitively() {
    let code = [
        ("a.rs", "rust"),
        ("a.go", "go"),
        ("a.ts", "typescript"),
        ("a.TSX", "typescript"),
        ("a.mts", "typescript"),
        ("a.cts", "typescript"),
        ("a.js", "javascript"),
        ("a.jsx", "javascript"),
        ("a.mjs", "javascript"),
        ("a.cjs", "javascript"),
        ("a.py", "python"),
        ("a.pyi", "python"),
        ("a.swift", "swift"),
        ("a.c", "c/c++"),
        ("a.cc", "c/c++"),
        ("a.cpp", "c/c++"),
        ("a.cxx", "c/c++"),
        ("a.h", "c/c++"),
        ("a.hpp", "c/c++"),
        ("a.hxx", "c/c++"),
    ];
    for (name, lang) in code {
        assert_eq!(kind_of(Path::new(name)), Kind::Code(lang), "{name}");
    }
    let text = [
        ("a.proto", "protobuf"),
        ("a.sql", "sql"),
        ("a.graphql", "graphql"),
        ("a.gql", "graphql"),
        ("a.json", "json"),
        ("a.yaml", "yaml"),
        ("a.yml", "yaml"),
        ("a.toml", "toml"),
        ("a.md", "markdown"),
        ("a.sh", "shell"),
        ("a.bash", "shell"),
        ("a.fish", "shell"),
        ("a.txt", "text"),
        ("a.csv", "text"),
        ("a.env", "text"),
    ];
    for (name, lang) in text {
        assert_eq!(kind_of(Path::new(name)), Kind::Text(lang), "{name}");
    }
    assert_eq!(kind_of(Path::new("logo.png")), Kind::Skip);
    assert_eq!(kind_of(Path::new("Makefile")), Kind::Skip);
}

#[test]
fn schema_and_label_follow_the_format_not_only_the_extension() {
    let swagger = "swagger : \"2.0\"\n";
    assert_eq!(
        schema_of(Path::new("a.yml"), swagger),
        Some(Schema::OpenApi)
    );
    assert_eq!(
        schema_of(Path::new("a.yml"), " openapi: 3\n"),
        Some(Schema::Yaml)
    );
    assert_eq!(
        schema_of(Path::new("a.json"), "{\"swagger\": \"2.0\"}"),
        Some(Schema::OpenApi)
    );
    assert_eq!(
        schema_of(Path::new("a.json"), "{\"openapifoo\": 1}"),
        Some(Schema::Json)
    );
    assert_eq!(schema_of(Path::new("a.gql"), ""), Some(Schema::GraphQl));
    assert_eq!(schema_of(Path::new("a.rs"), "openapi: 1"), None);
    assert_eq!(schema_of(Path::new("a.toml"), "x = 1"), None);

    assert_eq!(label(Path::new("a.json"), "{\"a\": 1}"), Some("json"));
    assert_eq!(
        label(Path::new("a.json"), "{\"openapi\": \"3\"}"),
        Some("openapi")
    );
    assert_eq!(label(Path::new("a.gql"), ""), Some("graphql"));
    assert_eq!(label(Path::new("a.rs"), ""), Some("rust"));
    assert_eq!(label(Path::new("a.proto"), ""), Some("protobuf"));
    assert_eq!(label(Path::new("a.png"), ""), None);
}

#[test]
fn a_mapping_key_is_followed_by_a_colon_inside_or_outside_quotes() {
    let cases: &[(&str, u32, usize, bool)] = &[
        ("order_id: 1", 1, 8, true),
        ("order_id : 1", 1, 8, true),
        ("\"order_id\": 1", 2, 8, true),
        ("\"order_id\" : 1", 2, 8, true),
        ("'order_id': 1", 2, 8, true),
        ("\"order_id\" 1", 2, 8, false),
        ("x: order_id", 4, 8, false),
        ("x: order_id,", 4, 8, false),
        ("order_id", 1, 8, false),
        ("order_id", 1, 80, false),
    ];
    for (line, col, len, want) in cases {
        assert_eq!(is_mapping_key(line, &occ(1, *col, *len)), *want, "{line:?}");
    }
}

#[test]
fn json_and_yaml_structure_means_a_key_and_openapi_strips_matching_quotes() {
    let text = "{\"order_id\": 1,\n \"x\": \"order_id\"}\n";
    assert!(is_structural(Schema::Json, text, &occ(1, 3, 8)));
    assert!(!is_structural(Schema::Json, text, &occ(2, 8, 8)));
    assert!(is_structural(Schema::Yaml, "order_id: 1\n", &occ(1, 1, 8)));
    assert!(!is_structural(Schema::Yaml, "k: order_id\n", &occ(1, 4, 8)));

    let spec = "a: 'order_id'\nb: \"order_id'\nc: [order_id, x]\nd: see order_id here\ne: order_id # note\n";
    let want = [true, false, true, false, true];
    for (n, want) in want.iter().enumerate() {
        let line = spec.lines().nth(n).unwrap();
        let col = line.find("order_id").unwrap() as u32 + 1;
        assert_eq!(
            is_structural(Schema::OpenApi, spec, &occ(n as u32 + 1, col, 8)),
            *want,
            "{line:?}"
        );
    }
}

#[test]
fn graphql_names_inside_strings_comments_and_descriptions_are_prose() {
    let sdl = "\"\"\"\ndoc orderId\n\"\"\"\ntype T { orderId: ID } # orderId\n";
    assert!(in_block_string(sdl, &occ(2, 5, 7)));
    assert!(!in_block_string(sdl, &occ(4, 10, 7)));
    assert!(!in_block_string(sdl, &occ(9, 1, 7)));
    assert!(!is_structural(Schema::GraphQl, sdl, &occ(2, 5, 7)));
    assert!(is_structural(Schema::GraphQl, sdl, &occ(4, 10, 7)));
    assert!(!is_structural(Schema::GraphQl, sdl, &occ(4, 26, 7)));
    let mut quoted = occ(4, 10, 7);
    quoted.in_string = true;
    assert!(!is_structural(Schema::GraphQl, sdl, &quoted));
}

#[test]
fn comments_in_yaml_and_json_are_found_before_an_occurrence() {
    // (path, text, line, col, expected)
    let cases: &[(&str, &str, u32, u32, bool)] = &[
        ("a.yaml", "a: 1 # order_id\n", 1, 8, true),
        ("a.yaml", "# note\norder_id: 1\n", 2, 1, false),
        ("a.yaml", "a: \"# no\" order_id\n", 1, 11, false),
        ("a.yaml", "a: \"\\\" # \" order_id\n", 1, 12, false),
        ("a.yaml", "a: 'x' # order_id\n", 1, 10, true),
        ("a.yaml", "a: 1\r\nb: 2 # order_id\r\n", 2, 8, true),
        ("a.yaml", "# c\r\norder_id: 1\r\n", 2, 1, false),
        ("a.yaml", "a: \"é\" # order_id\n", 1, 10, true),
        ("a.yaml", "a: 1 // order_id\n", 1, 9, false),
        ("a.json", "{\"a\": 1, // order_id\n", 1, 14, true),
        ("a.json", "// c\n{\"order_id\": 1}\n", 2, 3, false),
        ("a.json", "{\"a\": 1, /* order_id */}\n", 1, 14, true),
        ("a.json", "{\"a\": /* c */ \"order_id\": 1}\n", 1, 16, false),
        ("a.json", "{/* a * b order_id */}\n", 1, 14, true),
        (
            "a.json",
            "{\"a\": \"// x\", \"order_id\": 1}\n",
            1,
            15,
            false,
        ),
        ("a.json", "{\"a\": 1 # order_id\n", 1, 11, false),
        ("a.json", "/ order_id\n", 1, 3, false),
        ("a.json", "{\"a\": `x` // order_id\n", 1, 15, true),
        ("a.rs", "// order_id\n", 1, 4, false),
        ("a.yaml", "order_id: 1\n", 7, 1, false),
        ("a.yaml", "order_id: 1", 1, 99, false),
    ];
    for (path, text, line, col, want) in cases {
        assert_eq!(
            text_comment_before(Path::new(path), text, &occ(*line, *col, 8)),
            *want,
            "{path} {text:?} @{line}:{col}"
        );
    }
}

/// An escaped `\"""` inside a description does not end it, so a later mention in the same
/// description is still prose, and the field after the real end is not (#1452).
#[test]
fn an_escaped_triple_quote_does_not_end_a_description() {
    let sdl = "\"\"\"\nquote \\\"\"\" here\norderId stays prose\n\"\"\"\ntype T { orderId: ID }\n";
    assert!(in_block_string(sdl, &occ(3, 1, 7)), "after the escape");
    assert!(!in_block_string(sdl, &occ(5, 10, 7)), "after the real end");
    assert!(!is_structural(Schema::GraphQl, sdl, &occ(3, 1, 7)));
    assert!(is_structural(Schema::GraphQl, sdl, &occ(5, 10, 7)));
}

/// A JSON OpenAPI document is recognized by its root key wherever that key is (#1452).
#[test]
fn minified_json_openapi_is_openapi_whatever_the_key_order() {
    let json = Path::new("/w/api/spec.json");
    let doc = r#"{"info":{"title":"demo","version":"1"},"openapi":"3.1.0","paths":{}}"#;
    assert_eq!(schema_of(json, doc), Some(Schema::OpenApi));
    let nested = r#"{"info":{"openapi":"not the root"},"paths":{}}"#;
    assert_eq!(schema_of(json, nested), Some(Schema::Json));
    assert_eq!(
        schema_of(json, "{\n  \"swagger\": \"2.0\",\n"),
        Some(Schema::OpenApi)
    );
}
