/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `force` against plans that are incomplete (#446).
//!
//! `force` writes a plan the analyzer rejects. It does not write a plan that left something
//! behind: a reference that was not rewritten (a function used as a value, a call it could not
//! read), a position where the file does not say what the analyzer says, an implementation the
//! analyzer's answer does not place, or a file the change is checked against that cannot be read.
//! Each is reported by a dry run and refused by a write, forced or not, with every file as it was.
//! The base revision wrote each of them when forced (some without being forced).

use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};
use std::net::SocketAddr;

const CARGO: &str = "[package]\nname = \"mm\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

/// A gateway that answers `references` with `refs`, finds no errors, and answers the other
/// methods from `also`.
async fn gateway(refs: Value, also: Vec<(&'static str, Value)>) -> SocketAddr {
    ScriptedGateway::start(move |method, _params| match method {
        "textDocument/references" => refs.clone(),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => also
            .iter()
            .find(|(m, _)| *m == method)
            .map_or(Value::Null, |(_, v)| v.clone()),
    })
    .await
    .addr()
}

/// Every file is as `files` wrote it.
fn assert_untouched(ws: &Workspace, files: &[(&str, &str)], why: &str) {
    for (rel, text) in files {
        assert_eq!(ws.read(rel), *text, "{rel} was written: {why}");
    }
}

fn concat(parts: &[Value]) -> Value {
    Value::Array(
        parts
            .iter()
            .flat_map(|p| p.as_array().cloned().unwrap_or_default())
            .collect(),
    )
}

const EVEN: &str = "pub fn is_even(n: u32) -> bool {\n    n % 2 == 0\n}\n\npub fn f(n: u32) -> u32 {\n    if is_even(n) { 1 } else { 0 }\n}\n\npub fn h() -> fn(u32) -> bool {\n    is_even\n}\n";
const SCALE: &str = "pub fn scale(x: u32, factor: u32) -> u32 {\n    x * factor\n}\n\npub fn a() -> u32 {\n    scale(1, 2)\n}\n\npub fn b() -> fn(u32, u32) -> u32 {\n    scale\n}\n";
const HOME: &str = "pub fn render(text: &str) -> String {\n    let width = 80;\n    format!(\"{text}{width}\")\n}\n\npub fn caller() -> String {\n    render(\"x\")\n}\n\npub fn table() -> fn(&str) -> String {\n    render\n}\n";

/// A function used as a value is a reference no rewrite reached: inverted, it would return the
/// opposite predicate; with a parameter inlined or extracted, it would change type in a file
/// nothing checks. Each planner reports it, and a forced write is refused with the source intact.
/// The base revision wrote all three: the inversion even unforced, the others when forced.
#[tokio::test]
async fn force_does_not_write_past_a_function_used_as_a_value() {
    let files = [
        ("Cargo.toml", CARGO),
        (
            "src/lib.rs",
            "pub mod even;\npub mod home;\npub mod scale;\n",
        ),
        ("src/even.rs", EVEN),
        ("src/scale.rs", SCALE),
        ("src/home.rs", HOME),
    ];
    let ws = Workspace::new(&files);
    let root = ws.root();
    let mut written = Vec::new();

    let even = ws.path("src/even.rs");
    let remote = gateway(answers::locations(&even, &[(6, 8), (10, 5)]), Vec::new()).await;
    let plan =
        prod_code_mcp::invert_boolean::invert(remote, &root, &even, 1, 8, "is_odd", false, true)
            .await
            .expect("the dry run reports");
    assert!(
        plan.unmatched
            .iter()
            .any(|u| u.contains("src/even.rs:10:5")),
        "{:?}",
        plan.unmatched
    );
    for force in [false, true] {
        match prod_code_mcp::invert_boolean::invert(
            remote, &root, &even, 1, 8, "is_odd", true, force,
        )
        .await
        {
            Ok(done) => written.push(format!("invert (force {force}): applied {}", done.applied)),
            Err(err) => assert!(
                format!("{err:#}").contains("src/even.rs:10:5"),
                "invert: {err:#}"
            ),
        }
        assert_untouched(&ws, &files, &format!("invert, force {force}"));
    }

    let scale = ws.path("src/scale.rs");
    let remote = gateway(answers::locations(&scale, &[(6, 5), (10, 5)]), Vec::new()).await;
    match prod_code_mcp::inline_parameter::inline_parameter(
        remote, &root, &scale, 1, 22, true, true,
    )
    .await
    {
        Ok(done) => written.push(format!("inline parameter: applied {}", done.applied)),
        Err(err) => assert!(
            format!("{err:#}").contains("src/scale.rs:10:5"),
            "inline parameter: {err:#}"
        ),
    }
    assert_untouched(&ws, &files, "inline parameter, forced");

    let home = ws.path("src/home.rs");
    let symbols = json!([
        answers::document_symbol("render", 12, 1, 4, 8),
        answers::document_symbol("caller", 12, 6, 8, 8),
        answers::document_symbol("table", 12, 10, 12, 8),
    ]);
    let remote = gateway(
        answers::locations(&home, &[(7, 5), (11, 5)]),
        vec![("textDocument/documentSymbol", symbols)],
    )
    .await;
    match prod_code_mcp::extract_parameter::extract(
        remote,
        &root,
        &home,
        (2, 17),
        (2, 19),
        "width_limit",
        Some("usize"),
        false,
        true,
        true,
    )
    .await
    {
        Ok(done) => written.push(format!("extract parameter: applied {}", done.applied)),
        Err(err) => assert!(
            format!("{err:#}").contains("src/home.rs:11:5"),
            "extract parameter: {err:#}"
        ),
    }
    assert_untouched(&ws, &files, "extract parameter, forced");

    assert!(
        written.is_empty(),
        "written past a function used as a value:\n{}",
        written.join("\n")
    );
}

const MOD_LIB: &str = "pub mod a;\npub mod c;\n";
const MOD_A: &str = "pub mod b;\n";
const MOD_B: &str = "pub fn f() -> u32 {\n    1\n}\n";
const MOD_C: &str = "pub fn g() -> u32 {\n    crate::a::b::f()\n}\n";

/// A path to a module at a position where the file names something else (`g`, not `b`) is not a
/// path the move can respell. The base revision passed over it and wrote the move, forced or not;
/// now the plan names it and the write is refused with every file where it was.
#[tokio::test]
async fn a_module_move_does_not_pass_over_a_stale_position() {
    let files = [
        ("Cargo.toml", CARGO),
        ("src/lib.rs", MOD_LIB),
        ("src/a.rs", MOD_A),
        ("src/a/b.rs", MOD_B),
        ("src/c.rs", MOD_C),
    ];
    let ws = Workspace::new(&files);
    let root = ws.root();
    let c = ws.path("src/c.rs");
    // (2, 15) is the `b` of `crate::a::b::f()`; (1, 8) is `g`.
    let remote = gateway(answers::locations(&c, &[(2, 15), (1, 8)]), Vec::new()).await;
    for force in [false, true] {
        let mut plan = prod_code_mcp::move_module::move_module(
            remote,
            &root,
            &ws.path("src/a/b.rs"),
            &root.join("src/c/b.rs"),
        )
        .await
        .expect("the dry run reports");
        let written = plan.write(force);
        let Err(err) = written else {
            panic!(
                "the move was written past src/c.rs:1:8 (force {force}); src/c.rs now:\n{}",
                ws.read("src/c.rs")
            );
        };
        assert!(
            format!("{err:#}").contains("src/c.rs:1:8"),
            "force {force}: {err:#}"
        );
        assert_untouched(&ws, &files, &format!("a stale position, force {force}"));
        assert!(!root.join("src/c/b.rs").exists(), "force {force}");
    }
}

const ORDER: &str = "pub mod tax;\n\npub struct Order {\n    pub total: u32,\n}\n\nimpl Order {\n    pub fn price_with(&self, tax: &tax::Tax, extra: u32) -> u32 {\n        self.total + self.total * tax.rate / 100 + extra\n    }\n}\n\npub fn checkout(o: &Order, t: &tax::Tax) -> u32 {\n    o.price_with(t, 1)\n}\n";
const TAX: &str = "pub struct Tax {\n    pub rate: u32,\n}\n\nimpl Tax {\n    pub fn zero() -> Self {\n        Tax { rate: 0 }\n    }\n}\n";

/// A call of the moved method at a position where the file says something else cannot be
/// rewritten; forced, the base revision wrote the move and left whatever is there calling a
/// method that is gone. Now the write is refused, forced or not.
#[tokio::test]
async fn a_method_move_does_not_force_past_a_stale_position() {
    let files = [
        ("Cargo.toml", CARGO),
        ("src/lib.rs", ORDER),
        ("src/tax.rs", TAX),
    ];
    let ws = Workspace::new(&files);
    let root = ws.root();
    let (lib, tax) = (ws.path("src/lib.rs"), ws.path("src/tax.rs"));
    let (l, t) = (lib.clone(), tax.clone());
    let remote = ScriptedGateway::start(move |method, params| match method {
        "textDocument/definition" => answers::locations(&t, &[(1, 12)]),
        // The parameter `tax` (8:30) is used once in the body; the method is called once, and
        // (1, 1) is `pub mod tax;`.
        "textDocument/references"
            if params.pointer("/position/character").and_then(|c| c.as_u64()) == Some(29) =>
        {
            answers::locations(&l, &[(9, 35)])
        }
        "textDocument/references" => answers::locations(&l, &[(14, 7), (1, 1)]),
        "textDocument/documentSymbol" => json!([{
            "name": "price_with", "kind": 6,
            "range": { "start": { "line": 7, "character": 4 }, "end": { "line": 9, "character": 5 } },
            "selectionRange": { "start": { "line": 7, "character": 11 }, "end": { "line": 7, "character": 21 } }
        }]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await
    .addr();
    for force in [false, true] {
        let outcome =
            prod_code_mcp::move_method::move_method(remote, &root, &lib, 8, 12, "tax", true, force)
                .await;
        if let Ok(done) = &outcome {
            assert!(
                !done.applied,
                "the move was written past src/lib.rs:1:1 (force {force})"
            );
            assert!(
                done.render(20_000).contains("src/lib.rs:1"),
                "force {force}"
            );
        }
        assert_untouched(&ws, &files, &format!("a stale position, force {force}"));
    }
}

const SHAPES: &str = "pub trait Shape {\n    fn area(&self, scale: u32, unused: u32) -> u32;\n}\n\npub struct Circle(pub u32);\npub struct Square(pub u32);\n\nimpl Shape for Circle {\n    fn area(&self, scale: u32, _unused: u32) -> u32 {\n        3 * self.0 * self.0 * scale\n    }\n}\n\nimpl Shape for Square {\n    fn area(&self, scale: u32, unused: u32) -> u32 {\n        self.0 * self.0 * scale\n    }\n}\n\npub fn total(shapes: &[&dyn Shape]) -> u32 {\n    shapes.iter().map(|s| s.area(2, 7)).sum()\n}\n\npub fn one(c: &Circle) -> u32 {\n    c.area(1, tick()) + Shape::area(c, 3, 0)\n}\n\nfn tick() -> u32 {\n    0\n}\n";

/// Removing a trait method's parameter needs every implementation. An implementation entry
/// without a position, and a call at a position where the file says something else, are not
/// "one implementation fewer" or "a use that is not a call": forced, the base revision wrote a
/// trait whose `Square` still took the parameter, or cut an argument out of the wrong text. Now
/// the first stops the plan and the second stops the write, forced or not.
#[tokio::test]
async fn a_trait_parameter_is_not_removed_past_a_missing_implementation_or_a_stale_call() {
    let files = [("Cargo.toml", CARGO), ("src/lib.rs", SHAPES)];
    let ws = Workspace::new(&files);
    let root = ws.root();
    let lib = ws.path("src/lib.rs");
    let fn_at = SHAPES.match_indices("area").nth(1).unwrap().0;
    let calls = answers::locations(&lib, &[(9, 8), (15, 8), (21, 29), (25, 7), (25, 32)]);
    let cases = [
        (
            "an implementation without a position",
            concat(&[
                answers::locations(&lib, &[(9, 8)]),
                json!([{ "uri": answers::uri(&lib) }]),
            ]),
            calls.clone(),
        ),
        (
            "a call at a stale position",
            answers::locations(&lib, &[(9, 8), (15, 8)]),
            concat(&[calls.clone(), answers::locations(&lib, &[(2, 1)])]),
        ),
    ];
    for (what, impls, refs) in cases {
        let l = lib.clone();
        let remote = ScriptedGateway::start(move |method, _params| match method {
            "textDocument/definition" => answers::locations(&l, &[(1, 11)]),
            "textDocument/implementation" => impls.clone(),
            "textDocument/references" => refs.clone(),
            "textDocument/diagnostic" => answers::no_diagnostics(),
            _ => Value::Null,
        })
        .await
        .addr();
        let outcome =
            prod_code_mcp::trait_param::remove_parameter(remote, &root, &lib, fn_at, 1, true, true)
                .await;
        if let Ok(done) = &outcome {
            assert!(
                !done.applied,
                "{what}: the forced removal was written; src/lib.rs now:\n{}",
                ws.read("src/lib.rs")
            );
        }
        assert_untouched(&ws, &files, what);
    }
}

const TOTAL: &str = "pub fn total(v: &Vec<u32>) -> u32 {\n    v.as_ref().iter().sum()\n}\n";

/// A file a change is checked against that cannot be read here is not a file with no errors. One
/// outside the checkout is not opened before the analyzer is asked about it (#271), so the base
/// revision took whatever the analyzer said, dropped the text it could not read, and reported the
/// file checked and clean — directly, and through making a parameter generic whose only caller
/// was that file, which it then wrote. Now each is an error naming the file, and nothing is
/// written.
#[tokio::test]
async fn an_unreadable_file_to_check_is_not_a_clean_one() {
    let files = [
        ("Cargo.toml", CARGO),
        ("src/lib.rs", "pub mod total;\n"),
        ("src/total.rs", TOTAL),
    ];
    let ws = Workspace::new(&files);
    let root = ws.root();
    let outside = root.file_name().unwrap().to_string_lossy().into_owned();
    let gone = root.with_file_name(format!("{outside}-gone.rs"));
    assert!(
        !gone.exists() && !gone.starts_with(&root),
        "{}",
        gone.display()
    );
    let total = ws.path("src/total.rs");

    let remote = gateway(Value::Null, Vec::new()).await;
    let checked = prod_code_mcp::diagnostics::validate_texts(
        remote,
        &root,
        &[(total.clone(), TOTAL.to_string())],
        std::slice::from_ref(&gone),
    )
    .await;
    let Err(err) = checked else {
        panic!("{} was reported as checked: {checked:?}", gone.display());
    };
    let text = format!("{err:#}");
    assert!(
        text.contains("cannot read") && text.contains("gone.rs"),
        "{text}"
    );

    let remote = gateway(answers::locations(&gone, &[(1, 1)]), Vec::new()).await;
    let generified = prod_code_mcp::generify::generify(
        remote,
        &root,
        &total,
        1,
        8,
        "v",
        "AsRef<[u32]>",
        "T",
        true,
        false,
    )
    .await;
    let Err(err) = generified else {
        panic!(
            "generified without checking {}: {generified:?}",
            gone.display()
        );
    };
    let text = format!("{err:#}");
    assert!(
        text.contains("cannot read") && text.contains("gone.rs"),
        "{text}"
    );
    assert_untouched(&ws, &files, "an unreadable caller");
}
