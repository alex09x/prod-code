/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::support::*;
use prod_code_mcp::exec::layout_only;
use prod_code_testkit::Workspace;

#[tokio::test]
async fn formatter_and_same_os_no_change_controls() {
    let before = "fn f() { libc::foo(a, b); }";
    let formatted = "fn f() {\n    libc::foo(a, b);\n}\n";
    for (label, poly, command, after, node, expect_warning, expect_layout) in [
        (
            "rustfmt-whitespace",
            false,
            vec!["cargo", "fmt"],
            formatted,
            other_os(),
            false,
            true,
        ),
        (
            "arbitrary-command",
            false,
            vec!["fixture"],
            formatted,
            other_os(),
            true,
            false,
        ),
        (
            "structured-check",
            true,
            vec!["fixture"],
            formatted,
            other_os(),
            true,
            false,
        ),
        (
            "same-os",
            false,
            vec!["fixture"],
            "fn f() { libc::bar(); }",
            prod_code_protocol::platform(),
            false,
            false,
        ),
        (
            "same-bytes",
            false,
            vec!["fixture"],
            before,
            other_os(),
            false,
            false,
        ),
        (
            "same-bytes-poly",
            true,
            vec!["fixture"],
            before,
            other_os(),
            false,
            false,
        ),
        (
            "formatter-reorder",
            false,
            vec!["cargo", "fmt"],
            "fn f() { libc::foo(b, a); }",
            other_os(),
            true,
            false,
        ),
        (
            "formatter-tuple",
            false,
            vec!["cargo", "fmt"],
            "fn f() { libc::foo((a,), b); }",
            other_os(),
            true,
            false,
        ),
    ] {
        let ws = Workspace::new(&[("file.rs", before)]);
        let g = gateway(vec![delta("file.rs", Some(after))], node).await;
        let root = ws.root();
        let client = run(g.address, &root, poly, &command);
        let release = async {
            g.arrived.await.unwrap();
            g.release.send(()).unwrap();
        };
        let (out, ()) = tokio::join!(client, release);
        g.task.await.unwrap();
        let warning = out.platform_warning(&root);
        println!(
            "control {label}: before={before:?} after={after:?} pulled={:?} relaid={:?} kept={:?} warning={warning:?}",
            out.pulled_files, out.relaid_files, out.kept_files
        );
        assert_eq!(ws.read("file.rs"), after);
        assert_eq!(warning.is_some(), expect_warning, "{label}");
        assert_eq!(!out.relaid_files.is_empty(), expect_layout, "{label}");
        if before == after {
            assert!(out.pulled_files.is_empty());
        }
    }
}

#[test]
fn uncertain_tokens_literals_and_non_rust_changes_are_not_proven_layout() {
    for (before, after) in [
        ("f(a, b)", "f(b, a)"),
        ("(x)", "(x,)"),
        (
            "if (apple) { open(); cleanup(); }",
            "if (apple) open(); cleanup();",
        ),
        ("f(\"a b\")", "f(\"ab\")"),
        ("f(r#\"a b\"#)", "f(r#\"ab\"#)"),
        ("f(b\"x\")", "f(b \"x\")"),
        ("f('a')", "f('b')"),
        ("f(br#\"a\" b c\"#)", "f(br#\"a\" bc\"#)"),
        ("f(cr#\"a\" b c\"#)", "f(cr#\"a\" bc\"#)"),
        ("let a\u{301} = 1;", "let a \u{301} = 1;"),
        ("f(1e-3)", "f(1e - 3)"),
        ("a >> b", "a > > b"),
        ("a && b", "a & & b"),
        ("1.0", "1 . 0"),
        ("// evidence\nf()", "// evidence f()"),
        ("use b::B; use a::A;", "use a::A; use b::B;"),
        ("x.map(|p| { f(p) })", "x.map(|p| f(p))"),
    ] {
        assert!(
            !layout_only(before.as_bytes(), after.as_bytes()),
            "{before:?} -> {after:?}"
        );
    }
}
