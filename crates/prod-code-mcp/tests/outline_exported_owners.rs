/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! An exported-only outline counts each member's owners (#1418): a public Rust trait keeps its
//! required methods and a public enum its variants, while the members of a TypeScript class
//! that is not exported are left out with it. `outline_file` is the renderer the MCP tool and
//! the CLI share.

use prod_code_mcp::tools::{OutlineOptions, outline_file};
use prod_code_testkit::{ScriptedGateway, Workspace};

/// A nested `documentSymbol` node over `lines` (0-based, inclusive).
fn node(
    name: &str,
    kind: u32,
    lines: (u32, u32),
    children: Vec<serde_json::Value>,
) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "kind": kind,
        "range": { "start": { "line": lines.0, "character": 0 }, "end": { "line": lines.1, "character": 1 } },
        "selectionRange": { "start": { "line": lines.0, "character": 0 }, "end": { "line": lines.0, "character": 1 } },
        "children": children,
    })
}

async fn exported_outline(file: &str, source: &str, symbols: serde_json::Value) -> String {
    let ws = Workspace::new(&[(file, source)]);
    let root = ws.root();
    let gateway = ScriptedGateway::start(move |method, _params| match method {
        "textDocument/documentSymbol" => symbols.clone(),
        _ => serde_json::Value::Null,
    })
    .await;
    let mut options = OutlineOptions::all(usize::MAX, false, "include_locals");
    options.exported_only = true;
    outline_file(gateway.addr(), &root, &root.join(file), file, &options)
        .await
        .expect("the outline renders")
}

const RUST: &str = "pub trait Reader {\n    fn read(&self);\n}\npub enum Mode {\n    Fast,\n}\nenum Hidden {\n    Slow,\n}\npub struct Shown;\nimpl Shown {\n    pub fn make() -> Self { Shown }\n    fn private() {}\n}\n";

#[tokio::test]
async fn a_public_trait_keeps_its_methods_and_a_public_enum_its_variants() {
    let symbols = serde_json::json!([
        node("Reader", 11, (0, 2), vec![node("read", 12, (1, 1), vec![])]),
        node("Mode", 10, (3, 5), vec![node("Fast", 22, (4, 4), vec![])]),
        node("Hidden", 10, (6, 8), vec![node("Slow", 22, (7, 7), vec![])]),
        node("Shown", 23, (9, 9), vec![]),
        node(
            "impl Shown",
            19,
            (10, 13),
            vec![
                node("make", 6, (11, 11), vec![]),
                node("private", 6, (12, 12), vec![])
            ],
        ),
    ]);
    let text = exported_outline("src/lib.rs", RUST, symbols).await;
    for listed in ["Reader", "read", "Mode", "Fast", "Shown", "make"] {
        assert!(text.contains(&format!("] {listed} (")), "{listed}: {text}");
    }
    for hidden in ["Hidden", "Slow", "private"] {
        assert!(!text.contains(&format!("] {hidden} (")), "{hidden}: {text}");
    }
}

const TS: &str = "class Hidden {\n    run() {}\n}\nexport class Shown {\n    run() {}\n    private hide() {}\n}\n";

#[tokio::test]
async fn the_members_of_a_class_that_is_not_exported_are_left_out() {
    let symbols = serde_json::json!([
        node("Hidden", 5, (0, 2), vec![node("run", 6, (1, 1), vec![])]),
        node(
            "Shown",
            5,
            (3, 6),
            vec![
                node("run", 6, (4, 4), vec![]),
                node("hide", 6, (5, 5), vec![])
            ],
        ),
    ]);
    let text = exported_outline("src/lib.ts", TS, symbols).await;
    assert!(text.contains("[Class] Shown (line 4)"), "{text}");
    assert!(text.contains("[Method] run (line 5)"), "{text}");
    assert!(!text.contains("Hidden"), "{text}");
    assert!(
        !text.contains("(line 2)"),
        "Hidden's run is not exported: {text}"
    );
    assert!(!text.contains("hide"), "{text}");
}
