/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Go loops turned into functional literals, and the polyglot entry point that applies them:
//! every shape that is recognised and every reason one is refused.

use prod_code_mcp::loop_to_iterator::{loop_to_iterator_polyglot, recognise_go};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};

fn go(decl: &str, header: &str, body: &str) -> Result<String, String> {
    let src = format!("func f() {{\n    {decl}\n    for {header} {{\n        {body}\n    }}\n}}\n");
    recognise_go(&src, src.find("for ").unwrap())
        .map(|poly| poly.statement)
        .map_err(|e| e.to_string())
}

const RANGE: &str = "_, p := range prices";

#[test]
fn accumulating_loops_become_function_literals() {
    for (decl, header, body, expected) in [
        (
            "sum := 0",
            RANGE,
            "sum += p",
            "sum := func() int { s := 0; for _, p := range prices { s += p }; return s }()",
        ),
        (
            "sum := 0",
            "p := range prices",
            "if p > 0 {\n            sum += p\n        }",
            "sum := func() int { s := 0; for p := range prices { if p > 0 { s += p } }; return s }()",
        ),
        (
            "n := 0",
            RANGE,
            "if p > 0 {\n            n++\n        }",
            "n := func() int { c := 0; for _, p := range prices { if p > 0 { c++ } }; return c }()",
        ),
        (
            "n := 0",
            RANGE,
            "n++",
            "n := func() int { c := 0; for _, p := range prices { if true { c++ } }; return c }()",
        ),
        (
            "n := 0",
            RANGE,
            "if p > 0 {\n            n += 1\n        }",
            "n := func() int { c := 0; for _, p := range prices { if p > 0 { c++ } }; return c }()",
        ),
        (
            "var out []int",
            RANGE,
            "out = append(out, p*2)",
            "out := func() []interface{} { res := make([]interface{}, 0); for _, p := range prices { res = append(res, p*2) }; return res }()",
        ),
        (
            "var out = []int{}",
            RANGE,
            "if p > 1 {\n            out = append(out, p)\n        }",
            "out := func() []interface{} { res := make([]interface{}, 0); for _, p := range prices { if p > 1 { res = append(res, p) } }; return res }()",
        ),
        (
            "found := false",
            RANGE,
            "if p == 3 {\n            found = true\n            break\n        }",
            "found := func() bool { for _, p := range prices { if p == 3 { return true } }; return false }()",
        ),
        (
            "ok := true",
            RANGE,
            "if !p.valid {\n            ok = false\n            break\n        }",
            "ok := func() bool { for _, p := range prices { if !(p.valid) { return false } }; return true }()",
        ),
        (
            "ok := true",
            RANGE,
            "if p < 0 {\n            ok = false\n            break\n        }",
            "ok := func() bool { for _, p := range prices { if !(!(p < 0)) { return false } }; return true }()",
        ),
    ] {
        assert_eq!(
            go(decl, header, body).as_deref(),
            Ok(expected),
            "{decl} / {body}"
        );
    }
}

#[test]
fn a_loop_that_an_iterator_cannot_express_is_refused_with_its_reason() {
    let cannot_break = "the loop's body has `break`, which an iterator chain cannot express";
    let one_statement = |acc: &str| {
        format!("the loop's body uses `{acc}` for more than the one accumulating statement")
    };
    let not_accumulating = "the loop's body is not `sum += …` or `sum = append(…)`";
    for (decl, header, body, expected) in [
        (
            "sum := 0",
            "i := 0; i < 3; i++",
            "sum += i",
            "expected `range` in for loop".to_string(),
        ),
        (
            "sum := 0",
            "a, b, c := range xs",
            "sum += b",
            "Go range header must name one or two variables".to_string(),
        ),
        (
            "sum := 0",
            "i, p := range prices",
            "sum += i + p",
            "the loop body uses range index/key `i`, which the iterator conversion cannot preserve"
                .to_string(),
        ),
        (
            "work()",
            RANGE,
            "sum += p",
            "the statement above the loop is not a variable declaration".to_string(),
        ),
        (
            "a.b := 0",
            RANGE,
            "a.b += p",
            "`a.b` is not a single variable".to_string(),
        ),
        (
            "sum := 0",
            RANGE,
            "if p < 0 {\n            continue\n        }\n        sum += p",
            "the loop's body has `continue`, which an iterator chain cannot express".to_string(),
        ),
        (
            "sum := 0",
            RANGE,
            "if p < 0 {\n            return\n        }\n        sum += p",
            "the loop's body has `return`, which an iterator chain cannot express".to_string(),
        ),
        (
            "sum := 0",
            RANGE,
            "sum += p\n        break",
            cannot_break.to_string(),
        ),
        (
            "sum := 0",
            RANGE,
            "if p {\n sum += p\n break\n }\n break",
            cannot_break.to_string(),
        ),
        (
            "found := false",
            RANGE,
            "if p == 3 {\n            found = true\n            break\n        } else {\n            work()\n        }",
            "the `if` has an `else` or is followed by more statements".to_string(),
        ),
        (
            "n := 0",
            RANGE,
            "if p == 3 {\n            n = 5\n            break\n        }",
            cannot_break.to_string(),
        ),
        (
            "found := false",
            RANGE,
            "if p == 3 {\n            other = true\n            break\n        }",
            one_statement("found"),
        ),
        ("sum := 0", RANGE, "work()", one_statement("sum")),
        ("sum := 0", RANGE, "work(sum)", not_accumulating.to_string()),
        ("sum := 0", RANGE, "sum -= p", not_accumulating.to_string()),
        (
            "sum := 0",
            RANGE,
            "sum = p",
            "unsupported assignment in Go loop".to_string(),
        ),
        (
            "var out []int",
            RANGE,
            "out = append(other, p)",
            "append target is not accumulator".to_string(),
        ),
        (
            "var out []int",
            RANGE,
            "out = append(out, p",
            "malformed append call".to_string(),
        ),
        (
            "sum := 0",
            RANGE,
            "if p > 0 sum += p",
            "the `if` has no body".to_string(),
        ),
        (
            "var total = 5",
            RANGE,
            "total += p",
            "`total` starts at `5`, not zero, so a sum would lose it".to_string(),
        ),
        (
            "n := 3",
            RANGE,
            "n++",
            "`n` starts at `3`, not zero, so a count would lose it".to_string(),
        ),
    ] {
        assert_eq!(go(decl, header, body), Err(expected), "{decl} / {body}");
    }
}

#[test]
fn the_loop_is_found_near_the_position_and_its_edges_are_exact() {
    let src =
        "func f() {\n\tsum := 0\n\tfor _, p := range prices {\n\t\tsum += p\n\t}\n\treturn\n}\n";
    let start = src.find("\tsum").unwrap();
    let end = src.find("\t}\n\treturn").unwrap() + 2;
    // From the line above, from the line itself and from after the loop: the same loop.
    for at in [
        0,
        src.find("for ").unwrap() + 2,
        src.find("return").unwrap(),
    ] {
        let poly = recognise_go(src, at).unwrap();
        assert_eq!((poly.start, poly.end), (start, end), "at {at}");
        assert_eq!(poly.indent, "\t");
        assert_eq!(
            poly.replacement,
            "\tsum := func() int { s := 0; for _, p := range prices { s += p }; return s }()"
        );
    }
    let refuse = |text: &str, at: usize| recognise_go(text, at).unwrap_err().to_string();
    assert_eq!(
        refuse("func f() {}\n", 0),
        "no `for` loop found at or near this position"
    );
    assert_eq!(
        refuse("sum := 0\nfor _, p := range xs", 0),
        "the loop has no body `{`"
    );
    assert_eq!(
        refuse("sum := 0\nfor _, p := range xs {\n", 0),
        "the loop's body is not closed"
    );
}

const MAIN_GO: &str = "package main\n\nfunc total(prices []int) int {\n    sum := 0\n    for _, p := range prices {\n        sum += p\n    }\n    return sum\n}\n";
const MAIN_GO_AFTER: &str = "package main\n\nfunc total(prices []int) int {\n    sum := func() int { s := 0; for _, p := range prices { s += p }; return s }()\n    return sum\n}\n";

async fn gateway(diagnostics: serde_json::Value) -> ScriptedGateway {
    ScriptedGateway::start(move |method, _| match method {
        "textDocument/diagnostic" => diagnostics.clone(),
        _ => serde_json::Value::Null,
    })
    .await
}

#[tokio::test]
async fn the_entry_point_previews_applies_and_refuses_on_compile_errors() {
    let ws = Workspace::new(&[("main.go", MAIN_GO)]);
    let file = ws.path("main.go");
    let clean = gateway(answers::no_diagnostics()).await;
    let run = |gw: &ScriptedGateway, symbol, line, apply, force| {
        let (root, file, addr) = (ws.root(), file.clone(), gw.addr());
        async move {
            loop_to_iterator_polyglot(addr, &root, &file, symbol, line, None, apply, force).await
        }
    };

    let preview = run(&clean, Some("total"), None, false, false)
        .await
        .unwrap();
    assert_eq!(
        preview.statement,
        "sum := func() int { s := 0; for _, p := range prices { s += p }; return s }()"
    );
    assert_eq!(preview.file, "main.go");
    assert_eq!(preview.new_text, MAIN_GO_AFTER);
    assert!(preview.diagnostics.is_empty() && !preview.applied);
    assert_eq!(ws.read("main.go"), MAIN_GO);

    let broken = gateway(answers::error_at(4, 5, "E1", "boom\nmore")).await;
    let refused = run(&broken, None, Some(5), true, false).await.unwrap_err();
    assert_eq!(
        refused.to_string(),
        "the change does not compile (1 error(s)); nothing was written:\n  boom [E1] (main.go:4:5)"
    );
    assert_eq!(ws.read("main.go"), MAIN_GO);

    let forced = run(&broken, None, Some(5), true, true).await.unwrap();
    assert!(forced.applied);
    assert_eq!(forced.diagnostics, vec!["boom [E1] (main.go:4:5)"]);
    assert_eq!(ws.read("main.go"), MAIN_GO_AFTER);
}

#[tokio::test]
async fn files_it_cannot_convert_are_named() {
    let ws = Workspace::new(&[
        ("notes.txt", "for x in y\n"),
        ("Main.java", "class Main {}\n"),
        ("lib.rs", "fn f() {}\n"),
    ]);
    let gw = gateway(answers::no_diagnostics()).await;
    let refuse = |name: &str| {
        let file = ws.path(name);
        let (root, addr) = (ws.root(), gw.addr());
        async move {
            loop_to_iterator_polyglot(addr, &root, &file, None, None, None, false, false)
                .await
                .unwrap_err()
                .to_string()
        }
    };
    assert_eq!(
        refuse("notes.txt").await,
        format!(
            "unsupported language for {}",
            ws.path("notes.txt").display()
        )
    );
    assert_eq!(
        refuse("Main.java").await,
        "loop_to_iterator does not support Java yet"
    );
    assert_eq!(
        refuse("lib.rs").await,
        "missing `line` or `symbol` identifying the loop"
    );
    assert_eq!(
        refuse("gone.go").await,
        format!("cannot read {}", ws.path("gone.go").display())
    );
}

#[tokio::test]
async fn a_rust_file_is_found_by_line_and_column_by_line_alone_or_by_symbol() {
    let src = "fn total(prices: &[u64]) -> u64 {\n    let mut sum: u64 = 0;\n    for p in prices {\n        sum += p;\n    }\n    sum\n}\n";
    let ws = Workspace::new(&[("lib.rs", src)]);
    let file = ws.path("lib.rs");
    let gw = gateway(answers::no_diagnostics()).await;
    let mut statements = Vec::new();
    for (symbol, line, col) in [
        (None, Some(3), Some(5)),
        (None, Some(3), None),
        (Some("total"), None, None),
    ] {
        let done = loop_to_iterator_polyglot(
            gw.addr(),
            &ws.root(),
            &file,
            symbol,
            line,
            col,
            false,
            false,
        )
        .await
        .unwrap();
        assert_eq!(done.file, "lib.rs");
        assert!(!done.applied);
        statements.push(done.statement);
    }
    assert_eq!(
        statements[0],
        "    let sum: u64 = prices.into_iter().sum();"
    );
    assert_eq!(statements[1], statements[0]);
    assert_eq!(statements[2], statements[0]);
}
