/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Making a Rust method an associated function: the receiver-reading helpers, every refusal of
//! the declaration, and what happens to call sites that cannot be rewritten safely.

use prod_code_mcp::make_static::helpers::{display, is_ident, mentions};
use prod_code_mcp::make_static::{
    extract_receiver, find_method_at_line, make_static, receiver_has_effects, split_receiver,
};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::path::Path;

const CALC: &str = "pub struct Calc;\n\nimpl Calc {\n    pub fn double(&self, x: u32) -> u32 {\n        x * 2\n    }\n    pub fn show(&self) -> u32 {\n        self.id()\n    }\n    pub fn plain(x: u32) -> u32 {\n        x\n    }\n}\n\nimpl Default for Calc {\n    pub fn dflt(&self) -> u32 {\n        1\n    }\n}\n\npub fn free(&self) {}\n\npub fn run(c: &Calc) -> u32 {\n    let a = c.double(1);\n    let b = Calc::double(c, 2);\n    a + b\n}\n";
const CALLS: &str = "pub struct Calc;\nimpl Calc {\n    pub fn double(&self, x: u32) -> u32 {\n        x * 2\n    }\n}\npub fn run(c: &Calc) -> u32 {\n    let a = load()?.double(3);\n    let f = Calc::double;\n    let g = c.double(1);\n    let h = Calc::double();\n    let k = double(c, 1);\n    a\n}\n";

fn location(file: &Path, line: u32, character: u32) -> Value {
    json!({ "uri": format!("file://{}", file.display()), "range": {
        "start": { "line": line, "character": character },
        "end": { "line": line, "character": character + 6 } } })
}

async fn gateway(file: &Path, spots: &[(u32, u32)], diagnostics: Value) -> SocketAddr {
    let refs: Vec<Value> = spots.iter().map(|(l, c)| location(file, *l, *c)).collect();
    ScriptedGateway::start(move |method, _| match method {
        "textDocument/references" => Value::Array(refs.clone()),
        "textDocument/diagnostic" => diagnostics.clone(),
        _ => Value::Null,
    })
    .await
    .addr()
}

#[test]
fn receivers_are_split_from_parameter_lists_and_effects_are_spotted() {
    for (params, expected) in [
        ("&self, x: u32", Some(("&self", "x: u32"))),
        ("&mut self", Some(("&mut self", ""))),
        ("mut self, a: u8, b: u8", Some(("mut self", "a: u8, b: u8"))),
        ("&'a self, x: &'a str", Some(("&'a self", "x: &'a str"))),
        ("&'a mut self", Some(("&'a mut self", ""))),
        ("self: Box<Self>, y: u8", Some(("self: Box<Self>", "y: u8"))),
        ("x: u32", None),
        ("selfie: u8", None),
        ("&'a", None),
        ("", None),
    ] {
        assert_eq!(
            split_receiver(params),
            expected.map(|(r, p)| (r.to_string(), p.to_string())),
            "{params:?}"
        );
    }
    for (receiver, effects) in [
        ("self", false),
        ("  CONFIG  ", false),
        ("self.field", false),
        ("load()", true),
        ("x?", true),
        ("vec![1]", true),
        ("task.await", true),
        ("items[0]", true),
    ] {
        assert_eq!(receiver_has_effects(receiver), effects, "{receiver:?}");
    }
}

#[test]
fn words_receivers_and_declarations_are_read_from_text() {
    for (text, word, expected) in [
        ("self.x", "self", true),
        ("(self)", "self", true),
        ("myself", "self", false),
        ("selfish", "self", false),
        ("a self_b", "self", false),
        ("éself", "self", false),
        ("", "self", false),
    ] {
        assert_eq!(mentions(text, word), expected, "{text:?} {word:?}");
    }
    assert!(is_ident('é') && is_ident('_') && is_ident('9') && !is_ident('-'));
    let root = Path::new("/w");
    assert_eq!(display(root, Path::new("/w/src/lib.rs")), "src/lib.rs");
    assert_eq!(
        display(root, Path::new("/elsewhere/a.rs")),
        "/elsewhere/a.rs"
    );

    for (before, expected) in [
        ("let a = self.items", Some("self.items")),
        ("x = obj.get(1, 2).chain", Some("obj.get(1, 2).chain")),
        ("p->next", Some("p->next")),
        ("v[idx].len", Some("v[idx].len")),
        ("a + ", None),
        ("   ", None),
        ("", None),
    ] {
        assert_eq!(extract_receiver(before), expected, "{before:?}");
    }

    let python =
        "class A:\n    async def go(self):\n        pass\n    def run(self):\n        pass\n";
    assert_eq!(find_method_at_line(python, 2), Some(("go".into(), None)));
    assert_eq!(find_method_at_line(python, 5), Some(("run".into(), None)));
    let go = "func (s *Server) Handle(w int) {\n}\nfunc Free(x int) {\n}\n";
    assert_eq!(
        find_method_at_line(go, 1),
        Some(("Handle".into(), Some("Server".into())))
    );
    assert_eq!(find_method_at_line(go, 3), Some(("Free".into(), None)));
    let java = "class A {\n    public int add(int a, int b) {\n        if (a > b) {\n";
    assert_eq!(find_method_at_line(java, 2), Some(("add".into(), None)));
    assert_eq!(find_method_at_line("}\nif (x) {\n", 2), None);
    // The method must be within two lines above or below the asked line.
    let far = "def a():\n  x\n  y\n  z\n  w\n";
    assert_eq!(find_method_at_line(far, 3), Some(("a".into(), None)));
    assert_eq!(find_method_at_line(far, 4), None);
    assert_eq!(find_method_at_line(far, 0), None);
    assert_eq!(find_method_at_line(far, 99), None);
}

#[tokio::test]
async fn a_declaration_that_cannot_lose_its_receiver_is_refused_by_name() {
    let ws = Workspace::new(&[
        ("src/lib.rs", CALC),
        ("src/open.rs", "impl Z {\n    fn t(&self);\n}\n"),
    ]);
    let lib = ws.path("src/lib.rs");
    let remote = gateway(&lib, &[], answers::no_diagnostics()).await;
    for (line, col, message) in [
        (
            1,
            12,
            "the position is not the name of a method declaration",
        ),
        (99, 1, "the position is not inside the file"),
        (
            7,
            12,
            "`show` uses `self`; only a method that never does can lose its receiver",
        ),
        (
            10,
            12,
            "`plain` takes no `self`: it is already an associated function",
        ),
        (
            16,
            12,
            "`dflt` implements a trait method, and the trait decides whether it takes `self`; change the trait instead",
        ),
        (21, 8, "the method is not inside an `impl` block"),
    ] {
        let err = make_static(remote, &ws.root(), &lib, line, col, false, false)
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), message, "{line}:{col}");
    }
    let err = make_static(
        remote,
        &ws.root(),
        &ws.path("src/open.rs"),
        2,
        8,
        false,
        false,
    )
    .await
    .unwrap_err();
    assert_eq!(err.to_string(), "`t` has no body");
    let err = make_static(
        remote,
        &ws.root(),
        &ws.path("src/none.rs"),
        1,
        1,
        false,
        false,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().starts_with("cannot read "), "{err}");
}

#[tokio::test]
async fn calls_are_rewritten_in_both_shapes_and_only_written_on_apply() {
    let ws = Workspace::new(&[("src/lib.rs", CALC)]);
    let lib = ws.path("src/lib.rs");
    let remote = gateway(&lib, &[(23, 14), (24, 18)], answers::no_diagnostics()).await;
    let expected = CALC
        .replace("double(&self, x: u32)", "double(x: u32)")
        .replace("c.double(1)", "Calc::double(1)")
        .replace("Calc::double(c, 2)", "Calc::double(2)");

    let preview = make_static(remote, &ws.root(), &lib, 4, 12, false, false)
        .await
        .unwrap();
    assert_eq!(
        (
            preview.owner.as_str(),
            preview.method.as_str(),
            preview.receiver.as_str()
        ),
        ("Calc", "double", "&self")
    );
    assert_eq!(preview.file, "src/lib.rs");
    assert_eq!(preview.rewritten_calls, 2);
    assert!(preview.blocked.is_empty() && preview.unmatched.is_empty());
    assert!(preview.diagnostics.is_empty() && !preview.applied);
    assert_eq!(
        preview.rewritten,
        vec![(lib.to_string_lossy().into_owned(), expected.clone())]
    );
    assert_eq!(ws.read("src/lib.rs"), CALC);

    let applied = make_static(remote, &ws.root(), &lib, 4, 12, true, false)
        .await
        .unwrap();
    assert!(applied.applied);
    assert_eq!(ws.read("src/lib.rs"), expected);
}

#[tokio::test]
async fn a_call_that_cannot_be_rewritten_is_reported_and_blocks_the_write() {
    let ws = Workspace::new(&[("src/lib.rs", CALLS)]);
    let lib = ws.path("src/lib.rs");
    let spots = [
        (7, 20),
        (8, 18),
        (9, 14),
        (10, 18),
        (11, 12),
        (4, 4),
        (500, 0),
    ];
    let remote = gateway(&lib, &spots, answers::no_diagnostics()).await;

    let preview = make_static(remote, &ws.root(), &lib, 3, 12, false, false)
        .await
        .unwrap();
    assert_eq!(preview.rewritten_calls, 1);
    assert_eq!(
        preview.blocked,
        vec!["src/lib.rs:8:21 `load()?` is evaluated for what it does"]
    );
    assert_eq!(
        preview.unmatched,
        vec![
            "src/lib.rs:9:19 (not a call: the method used as a value)",
            "src/lib.rs:11:19 (a call with no receiver argument)",
            "src/lib.rs:12:13 (neither a method call nor a path call)",
            "src/lib.rs:5:5 (the analyzer places `double` here, but the file says otherwise)",
            "src/lib.rs:501:1 (the position is not in the file)",
        ]
    );
    let blocked = make_static(remote, &ws.root(), &lib, 3, 12, true, false)
        .await
        .unwrap_err();
    assert_eq!(
        blocked.to_string(),
        "1 call site(s) would drop a receiver that does something; nothing was written:\n  src/lib.rs:8:21 `load()?` is evaluated for what it does"
    );
    // `force` accepts the dropped receiver, not a reference that was left unrewritten.
    let left = make_static(remote, &ws.root(), &lib, 3, 12, true, true)
        .await
        .unwrap_err();
    assert!(
        left.to_string().starts_with(
            "5 reference(s) to `double` were not rewritten, and would still pass a receiver; nothing was written:\n  src/lib.rs:9:19"
        ),
        "{left}"
    );
    assert_eq!(ws.read("src/lib.rs"), CALLS);
}

#[tokio::test]
async fn forcing_drops_an_effectful_receiver_and_compile_errors_block_the_write() {
    let ws = Workspace::new(&[("src/lib.rs", CALLS)]);
    let lib = ws.path("src/lib.rs");
    let remote = gateway(&lib, &[(7, 20), (9, 14)], answers::no_diagnostics()).await;
    let forced = make_static(remote, &ws.root(), &lib, 3, 12, true, true)
        .await
        .unwrap();
    assert_eq!((forced.rewritten_calls, forced.applied), (2, true));
    assert_eq!(forced.blocked.len(), 1);
    let written = ws.read("src/lib.rs");
    assert!(
        written.contains("    let a = Calc::double(3);\n"),
        "{written}"
    );
    assert!(
        written.contains("    let g = Calc::double(1);\n"),
        "{written}"
    );

    let ws = Workspace::new(&[("src/lib.rs", CALLS)]);
    let lib = ws.path("src/lib.rs");
    let failing = answers::error_at(10, 13, "E0425", "cannot find value `load`\nmore");
    let remote = gateway(&lib, &[(9, 14)], failing).await;
    let refused = make_static(remote, &ws.root(), &lib, 3, 12, true, false)
        .await
        .unwrap_err();
    assert!(
        refused.to_string().starts_with(
            "the change does not compile (1 error(s)); nothing was written. Pass `force: true` to write it anyway:\n  cannot find value `load` [E0425] ("
        ),
        "{refused}"
    );
    assert_eq!(ws.read("src/lib.rs"), CALLS);
    let forced = make_static(remote, &ws.root(), &lib, 3, 12, true, true)
        .await
        .unwrap();
    assert!(forced.applied && forced.diagnostics.len() == 1);
}
