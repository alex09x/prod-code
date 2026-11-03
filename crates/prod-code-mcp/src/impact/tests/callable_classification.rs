/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::symbols::collect_functions;

#[test]
fn local_let_inside_closure_is_not_collected_as_function() {
    let text = "fn check() {\n    let handler = || {\n        let frame_hint = 1;\n        frame_hint\n    };\n}\n";
    // Flat DocumentSymbols (as rust-analyzer emits for SymbolInformation)
    let symbols = serde_json::json!([
        {
            "name": "check",
            "kind": 12,
            "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 5, "character": 1 } },
            "selectionRange": { "start": { "line": 0, "character": 3 }, "end": { "line": 0, "character": 8 } }
        },
        {
            "name": "handler",
            "kind": 13,
            "detail": "impl Fn() -> i32",
            "containerName": "check",
            "range": { "start": { "line": 1, "character": 8 }, "end": { "line": 4, "character": 5 } },
            "selectionRange": { "start": { "line": 1, "character": 8 }, "end": { "line": 1, "character": 15 } }
        },
        {
            "name": "frame_hint",
            "kind": 13,
            "detail": "i32",
            "containerName": "handler",
            "range": { "start": { "line": 2, "character": 8 }, "end": { "line": 2, "character": 27 } },
            "selectionRange": { "start": { "line": 2, "character": 12 }, "end": { "line": 2, "character": 22 } }
        }
    ]);
    let mut functions = Vec::new();
    collect_functions(
        symbols.as_array().unwrap(),
        Some(text),
        "rust",
        &mut functions,
    )
    .unwrap();

    assert_eq!(
        functions
            .iter()
            .map(|(name, ..)| name.as_str())
            .collect::<Vec<_>>(),
        ["check"]
    );
}

#[test]
fn nested_fn_declaration_is_preserved_as_callable() {
    let text =
        "fn check() {\n    fn nested_helper() -> i32 { 42 }\n    let x = nested_helper();\n}\n";
    let symbols = serde_json::json!([
        {
            "name": "check",
            "kind": 12,
            "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 3, "character": 1 } },
            "selectionRange": { "start": { "line": 0, "character": 3 }, "end": { "line": 0, "character": 8 } }
        },
        {
            "name": "nested_helper",
            "kind": 12,
            "range": { "start": { "line": 1, "character": 4 }, "end": { "line": 1, "character": 37 } },
            "selectionRange": { "start": { "line": 1, "character": 7 }, "end": { "line": 1, "character": 20 } }
        }
    ]);
    let mut functions = Vec::new();
    collect_functions(
        symbols.as_array().unwrap(),
        Some(text),
        "rust",
        &mut functions,
    )
    .unwrap();

    assert_eq!(
        functions
            .iter()
            .map(|(name, ..)| name.as_str())
            .collect::<Vec<_>>(),
        ["check", "nested_helper"]
    );
}

#[test]
fn struct_field_named_function_is_not_classified_as_function() {
    let text = "pub struct Block {\n    pub function: Range<u64>,\n}\n";
    let symbols = serde_json::json!([
        {
            "name": "Block",
            "kind": 23,
            "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 2, "character": 1 } },
            "selectionRange": { "start": { "line": 0, "character": 11 }, "end": { "line": 0, "character": 16 } },
            "children": [
                {
                    "name": "function",
                    "kind": 8,
                    "detail": "Range<u64>",
                    "range": { "start": { "line": 1, "character": 4 }, "end": { "line": 1, "character": 29 } },
                    "selectionRange": { "start": { "line": 1, "character": 8 }, "end": { "line": 1, "character": 16 } }
                }
            ]
        }
    ]);
    let mut functions = Vec::new();
    collect_functions(
        symbols.as_array().unwrap(),
        Some(text),
        "rust",
        &mut functions,
    )
    .unwrap();

    assert!(
        functions.is_empty(),
        "struct field must not be collected as function: {functions:?}"
    );
}

#[test]
fn real_method_named_function_is_collected() {
    let text = "impl Block {\n    pub fn function(&self) -> Range<u64> { 0..1 }\n}\n";
    let symbols = serde_json::json!([
        {
            "name": "Block",
            "kind": 23,
            "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 2, "character": 1 } },
            "selectionRange": { "start": { "line": 0, "character": 5 }, "end": { "line": 0, "character": 10 } },
            "children": [
                {
                    "name": "function",
                    "kind": 6,
                    "detail": "fn(&self) -> Range<u64>",
                    "range": { "start": { "line": 1, "character": 4 }, "end": { "line": 1, "character": 50 } },
                    "selectionRange": { "start": { "line": 1, "character": 11 }, "end": { "line": 1, "character": 19 } }
                }
            ]
        }
    ]);
    let mut functions = Vec::new();
    collect_functions(
        symbols.as_array().unwrap(),
        Some(text),
        "rust",
        &mut functions,
    )
    .unwrap();

    assert_eq!(
        functions
            .iter()
            .map(|(name, ..)| name.as_str())
            .collect::<Vec<_>>(),
        ["function"]
    );
}

fn names(symbols: serde_json::Value, text: &str, language: &str) -> Vec<String> {
    let mut functions = Vec::new();
    collect_functions(
        symbols.as_array().unwrap(),
        Some(text),
        language,
        &mut functions,
    )
    .unwrap();
    functions.into_iter().map(|(name, ..)| name).collect()
}

fn function_symbol(name: &str, kind: u64, line: u64, start: u64, end: u64) -> serde_json::Value {
    let width = name.encode_utf16().count() as u64;
    serde_json::json!({
        "name": name,
        "kind": kind,
        "range": { "start": { "line": line, "character": 0 }, "end": { "line": line, "character": end } },
        "selectionRange": {
            "start": { "line": line, "character": start },
            "end": { "line": line, "character": start + width }
        }
    })
}

#[test]
fn a_function_after_non_bmp_text_is_found_by_its_utf16_column() {
    // `f` starts at UTF-16 unit 12: the emoji is two units but one char, so counting chars
    // lands on `(` instead.
    let text = "/* \u{1F600} */ fn f() {}\n";
    let column = text[..text.find("f(").unwrap()].encode_utf16().count() as u64;
    assert_eq!(column, 12);
    let symbols = serde_json::json!([function_symbol("f", 12, 0, column, 20)]);
    assert_eq!(names(symbols, text, "rust"), ["f"]);
}

#[test]
fn a_go_function_named_fn_is_collected() {
    let text = "package main\n\nfunc fn() int { return 1 }\n";
    let symbols = serde_json::json!([function_symbol("fn", 12, 2, 5, 26)]);
    assert_eq!(names(symbols, text, "go"), ["fn"]);
}

#[test]
fn javascript_callables_are_collected_however_rust_would_parse_them() {
    let text = "function fn() { return 1; }\nconst fn2 = () => 1;\nconst fn = (x) => x;\nlet run = function () { return 2; };\n";
    let mut arrow = function_symbol("fn", 14, 2, 6, 20);
    arrow["detail"] = serde_json::json!("(x: any) => any");
    let mut assigned = function_symbol("run", 13, 3, 4, 36);
    assigned["detail"] = serde_json::json!("() => number");
    let symbols = serde_json::json!([
        function_symbol("fn", 12, 0, 9, 27),
        function_symbol("fn2", 14, 1, 6, 20),
        arrow,
        assigned
    ]);
    assert_eq!(
        names(symbols, text, "typescript"),
        ["fn", "fn2", "fn", "run"]
    );
}
