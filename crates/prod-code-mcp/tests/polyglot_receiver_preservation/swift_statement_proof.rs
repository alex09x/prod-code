/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::to_method::convert_to_method_polyglot;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};

const BASE: &str = "class Base { typealias Void = String }\n";

#[tokio::test]
async fn review17_swift_regex_keywords_cannot_prove_statements() {
    let base = "class Base { typealias Void = Any }\n";
    let mut observations = Vec::new();
    for hashes in ["#", "##"] {
        for keyword in ["return", "let", "if", "# /#\nreturn"] {
            let before = format!(
                "class C: Base {{\n    static func same(c: C) -> Void {{\n        {hashes}/\nfoo(a)\nbar(b)\n{keyword}\n/{hashes}\n    }}\n}}\nC.same(c: C())\n"
            );
            for force in [false, true] {
                let ws = Workspace::new(&[
                    ("base.swift", base),
                    ("main.swift", &before),
                    ("caller.txt", "untouched"),
                ]);
                let gateway = ScriptedGateway::start(|method, _| match method {
                    "textDocument/diagnostic" => answers::no_diagnostics(),
                    _ => serde_json::Value::Null,
                })
                .await;
                let path = ws.root().join("main.swift");
                let result = convert_to_method_polyglot(
                    gateway.addr(),
                    &ws.root(),
                    &path,
                    Some("C"),
                    "same",
                    true,
                    force,
                )
                .await;
                let after = std::fs::read_to_string(&path).unwrap();
                let error = result.as_ref().err().map(ToString::to_string);
                eprintln!(
                    "BASE:\n{base}\nBEFORE (force={force}):\n{before}\nRESULT: {error:?}\nAFTER:\n{after}"
                );
                assert_eq!(
                    std::fs::read_to_string(ws.root().join("base.swift")).unwrap(),
                    base
                );
                assert_eq!(
                    std::fs::read_to_string(ws.root().join("caller.txt")).unwrap(),
                    "untouched"
                );
                observations.push((before.clone(), after, error));
            }
        }
    }
    for (before, after, error) in observations {
        assert!(error.is_some_and(|e| e.contains("unsupported")), "{after}");
        assert_eq!(after, before);
    }
}

#[tokio::test]
async fn review17_swift_regex_arguments_keep_external_unit_statements() {
    let mut observations = Vec::new();
    for regex in [
        "#/abc/#",
        "##/abc/##",
        "#/\nfoo(a)\nbar(b)\nreturn\n/#",
        "#/\"return\"/#",
        r"#/a\/b/#",
        "##/abc/#return/##",
    ] {
        let body = format!("print({regex})\n        print(c)");
        let before = format!(
            "class C: Base {{\n    static func same(c: C) -> Void {{\n        {body}\n    }}\n}}\nC.same(c: C())\n"
        );
        for force in [false, true] {
            let ws = Workspace::new(&[("base.swift", "class Base {}\n"), ("main.swift", &before)]);
            let gateway = ScriptedGateway::start(|method, _| match method {
                "textDocument/diagnostic" => answers::no_diagnostics(),
                _ => serde_json::Value::Null,
            })
            .await;
            let path = ws.root().join("main.swift");
            let result = convert_to_method_polyglot(
                gateway.addr(),
                &ws.root(),
                &path,
                Some("C"),
                "same",
                true,
                force,
            )
            .await;
            let after = std::fs::read_to_string(&path).unwrap();
            let error = result.as_ref().err().map(ToString::to_string);
            eprintln!(
                "BASE:\nclass Base {{}}\nBEFORE (force={force}):\n{before}\nRESULT: {error:?}\nAFTER:\n{after}"
            );
            observations.push((body.clone(), after, error));
        }
    }
    for (body, after, error) in observations {
        assert!(error.is_none(), "{error:?}");
        assert!(after.contains("let c: C = self;"), "{after}");
        assert!(after.contains(&body), "{after}");
        assert!(!after.contains("return print("), "{after}");
    }
}

#[tokio::test]
async fn review16_swift_extended_regex_is_one_value_expression() {
    let base = "class Base { typealias Void = Any }\n";
    for hashes in ["#", "##"] {
        let before = format!(
            "class C: Base {{\n    static func same(c: C) -> Void {{\n        {hashes}/\nfoo(a)\nbar(b)\n/{hashes}\n    }}\n}}\nprint(C.same(c: C()) is Regex<AnyRegexOutput>)\n"
        );
        let mut observations = Vec::new();
        for force in [false, true] {
            let ws = Workspace::new(&[
                ("base.swift", base),
                ("main.swift", &before),
                ("caller.txt", "untouched"),
            ]);
            let gateway = ScriptedGateway::start(|method, _| match method {
                "textDocument/diagnostic" => answers::no_diagnostics(),
                _ => serde_json::Value::Null,
            })
            .await;
            let path = ws.root().join("main.swift");
            eprintln!("BASE:\n{base}\nBEFORE (force={force}):\n{before}");
            let result = convert_to_method_polyglot(
                gateway.addr(),
                &ws.root(),
                &path,
                Some("C"),
                "same",
                true,
                force,
            )
            .await;
            let after = std::fs::read_to_string(&path).unwrap();
            let error = result.as_ref().err().map(ToString::to_string);
            eprintln!("RESULT (force={force}): {error:?}\nAFTER:\n{after}");
            observations.push((error, after));
            assert_eq!(
                std::fs::read_to_string(ws.root().join("base.swift")).unwrap(),
                base
            );
            assert_eq!(
                std::fs::read_to_string(ws.root().join("caller.txt")).unwrap(),
                "untouched"
            );
        }
        for (error, after) in observations {
            assert!(
                error.is_some_and(|e| e.contains("unsupported")),
                "conversion must refuse unproved multiline result"
            );
            assert_eq!(after, before);
        }
    }
}

#[tokio::test]
async fn review16_swift_regex_return_and_explicit_unit_controls() {
    let base = "class Base { typealias Void = Any }\n";
    for (result, body, needs_return) in [
        ("Void", "#/foo(a)bar(b)/#", true),
        ("Void", "#/return/#", true),
        ("Void", "#/\"return\" let/#", true),
        ("Void", "#/return; let/#", true),
        (
            "Void",
            "print(c)\n        return #/\nfoo(a)\nbar(b)\nreturn\n/#",
            false,
        ),
        (
            "()",
            "print(#/\nfoo(a)\nbar(b)\n/#)\n        print(c)",
            false,
        ),
    ] {
        let before = format!(
            "class C: Base {{\n    static func same(c: C) -> {result} {{\n        {body}\n    }}\n}}\nC.same(c: C())\n"
        );
        for force in [false, true] {
            let after = convert_with_external_base(&before, force, base).await;
            assert!(after.contains(body), "{after}");
            assert_eq!(
                after.contains(&format!("return {body}")),
                needs_return,
                "{after}"
            );
        }
    }
}

#[tokio::test]
async fn review15_swift_interpolation_is_one_value_expression() {
    if !super::available("swiftc") {
        eprintln!("Swift compiler unavailable; no Swift runtime proof");
    }
    for (expression, expected) in [
        (r#""\(String(c.value));\(String(c.value))""#, "7;7\n"),
        (r##"#"\#(String(c.value));\#(String(c.value))"#"##, "7;7\n"),
        (
            r#""\({ String(c.value); return String(c.value) }());\(String(c.value))""#,
            "7;7\n",
        ),
    ] {
        let before = format!(
            "class C: Base {{\n    var value = 7\n    static func same(c: C) -> Void {{\n        {expression}\n    }}\n}}\nprint(C.same(c: C()))\n"
        );
        for force in [false, true] {
            let after = convert_with_external_base(&before, force, BASE).await;
            assert!(after.contains(&format!("return {expression}")), "{after}");
            if super::available("swiftc") {
                assert_eq!(compile_with_base(&before), expected);
                assert_eq!(compile_with_base(&after), expected);
            }
        }
    }
}

#[tokio::test]
async fn review15_swift_multiline_interpolation_keeps_implicit_return_guard() {
    let before = "class C: Base {\n    var value = 7\n    static func same(c: C) -> Void {\n        \"\"\"\n\\(String(c.value))\n\\(String(c.value))\n\"\"\"\n    }\n}\n";
    super::super::support::refuses(before, "swift").await;
}

async fn convert_with_external_base(before: &str, force: bool, base: &str) -> String {
    let ws = Workspace::new(&[("base.swift", base), ("main.swift", before)]);
    let gateway = ScriptedGateway::start(|method, _| match method {
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    })
    .await;
    let path = ws.root().join("main.swift");
    eprintln!("BASE:\n{base}\nBEFORE (force={force}):\n{before}");
    let result = convert_to_method_polyglot(
        gateway.addr(),
        &ws.root(),
        &path,
        Some("C"),
        "same",
        true,
        force,
    )
    .await
    .unwrap();
    let after = &result.rewritten[0].1;
    eprintln!("AFTER:\n{after}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), *after);
    assert_eq!(
        std::fs::read_to_string(ws.root().join("base.swift")).unwrap(),
        base
    );
    assert!(
        after.contains("let c: C") && after.contains(" = self;"),
        "{after}"
    );
    after.clone()
}

fn compile_with_base(source: &str) -> String {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("base.swift");
    let main = dir.path().join("main.swift");
    let binary = dir.path().join("case");
    std::fs::write(&base, BASE).unwrap();
    std::fs::write(&main, source).unwrap();
    let output = std::process::Command::new("swiftc")
        .args([&base, &main])
        .arg("-o")
        .arg(&binary)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = std::process::Command::new(binary).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[tokio::test]
async fn review15_swift_string_arguments_and_comments_keep_unit_statements() {
    for first in [
        r#"print("\(c.value)")"#,
        r##"print(#"\#(c.value)"#)"##,
        "print(\"\"\"\n\\(c.value)\n\"\"\")",
        "print(c.value) /* \" ; */",
        "print(c.value) // \" ;",
        "print(c.value) /* outer /* \" */ \" */",
    ] {
        let body = format!("{first}\n        print(c.value)");
        let before = format!(
            "class C: CustomStringConvertible {{\n    var description: String {{ \"C\" }}\n    var value = 7\n    static func same(c: C) -> Swift.Void {{\n        {body}\n    }}\n}}\nC.same(c: C())\n"
        );
        eprintln!("BEFORE:\n{before}");
        let (after, _) = super::convert(&before, "swift", true).await;
        eprintln!("AFTER:\n{after}");
        assert!(after.contains(&body), "{after}");
        assert!(!after.contains("return print("), "{after}");
        if super::available("swiftc") {
            assert_eq!(super::compiled("swiftc", "swift", &before), "7\n7\n");
            assert_eq!(super::compiled("swiftc", "swift", &after), "7\n7\n");
        }
    }
}
