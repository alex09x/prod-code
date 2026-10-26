/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::support::{available, compiled, convert, refuses};

#[tokio::test]
async fn review8_swift_shadowed_void_keeps_implicit_value_return() {
    let before = "class C {\n    typealias Void = Int\n    var value = 7\n    static func same(c: C) -> Void { c.value }\n}\nprint(C.same(c: C()))\n";
    eprintln!("BEFORE:\n{before}");
    let (after, _) = convert(before, "swift", true).await;
    eprintln!("AFTER:\n{after}");
    assert!(after.contains("return c.value"), "{after}");
    runtime_control(before, &after);
}

#[tokio::test]
async fn review8_swift_control_expression_nested_returns_refuse_atomically() {
    for body in [
        "if c.value > 0 { ({ () -> Int in return c.value })() } else { 0 }",
        "switch c.value { case 7: ({ () -> Int in return c.value })(); default: 0 }",
        "if c.value > 0 { [c].map { return $0.value }[0] } else { 0 }",
    ] {
        let before = format!(
            "class C {{\n    var value = 7\n    static func same(c: C) -> Int {{ {body} }}\n}}\nprint(C.same(c: C()))\n"
        );
        eprintln!("BEFORE:\n{before}");
        if available("swiftc") {
            assert_eq!(compiled("swiftc", "swift", &before), "7\n");
        } else {
            eprintln!(
                "Swift compiler unavailable; public API source controls only; no Swift runtime proof"
            );
        }
        if prod_code_mcp::to_method::to_method_swift(&before, Some("C"), "same").is_ok() {
            let (after, _) = convert(&before, "swift", true).await;
            eprintln!("UNSAFE ACCEPTED AFTER:\n{after}");
        }
        refuses(&before, "swift").await;
        eprintln!("Control expression refused before writes, both force settings");
    }
}

#[tokio::test]
async fn review8_swift_explicit_control_returns_keep_nested_captures() {
    for body in [
        "if c.value > 0 { return ({ () -> Int in return c.value })() } else { return 0 }",
        "switch c.value { case 7: return ({ () -> Int in return c.value })(); default: return 0 }",
        "if c.value > 0 { if c.value == 7 { return c.value } else { return 0 } } else { return 0 }",
    ] {
        let before = format!(
            "class C {{\n    var value = 7\n    static func same(c: C) -> Int {{ {body} }}\n}}\nprint(C.same(c: C()))\n"
        );
        let (after, _) = convert(&before, "swift", true).await;
        eprintln!("BEFORE:\n{before}AFTER:\n{after}");
        assert!(after.contains(body), "{after}");
        assert!(after.contains("let c: C = self;"), "{after}");
        runtime_control(&before, &after);
    }
}

#[tokio::test]
async fn review7_swift_void_calls_with_nested_returns_remain_statement_bodies() {
    for result in ["", " -> Void", " -> ()", " -> Swift.Void"] {
        for body in [
            "print(({ return c.value })())\n        print(c.value)",
            "print(c.value)\n        print(c.value)",
        ] {
            let before = format!(
                "class C {{\n    var value = 7\n    static func same(c: C){result} {{\n        {body}\n    }}\n}}\nC.same(c: C())\n"
            );
            eprintln!("BEFORE:\n{before}");
            let (after, _) = convert(&before, "swift", true).await;
            eprintln!("AFTER:\n{after}");
            assert!(after.contains("let c: C = self;"), "{after}");
            assert!(after.contains(body), "{after}");
            assert!(!after.contains("return print("), "{after}");
            runtime_control(&before, &after);
        }
    }
}

#[tokio::test]
async fn review6_swift_local_function_capture_is_statement_body() {
    let before = "class C {\n    var value = 7\n    static func same(c: C) {\n        func read() -> Int { return c.value }\n        print(read())\n    }\n}\nC.same(c: C())\n";
    eprintln!("BEFORE:\n{before}");
    let (after, _) = convert(before, "swift", true).await;
    eprintln!("AFTER:\n{after}");
    assert!(after.contains("let c: C = self;"), "{after}");
    assert!(after.contains("func read() -> Int { return c.value }"));
    assert!(after.contains("print(read())"));
    assert!(!after.contains("return func"), "{after}");
    runtime_control(before, &after);
}

#[tokio::test]
async fn review5_swift_single_expression_closure_with_semicolons() {
    for expression in [
        "({ let x = c; return x.value })()",
        "({ () -> Int in let x = c; return x.value })()",
        "({ let text = \"; return\"; return c.value + text.count })()",
        "c.`return`",
    ] {
        let before = format!(
            "class C {{\n    var value = 7\n    var `return` = 7\n    static func same(c: C) -> Int {{ {expression} }}\n}}\nprint(C.same(c: C()))\n"
        );
        let (after, _) = convert(&before, "swift", true).await;
        eprintln!("BEFORE:\n{before}AFTER:\n{after}");
        assert!(after.contains(&format!("return {expression}")), "{after}");
        runtime_control(&before, &after);
    }
}

#[tokio::test]
async fn review5_swift_calls_before_explicit_method_return() {
    for body in [
        "print(c.value)\n        return c.value",
        "print(c.value); return c.value",
        "print(({ return c.value })())\n        return c.value",
        "print(\"return;\") // return c.value\n        return c.value",
        "print(c.value)\n        if c.value > 0 { return c.value } else { return 0 }",
    ] {
        let before = format!(
            "class C {{\n    var value = 7\n    static func same(c: C) -> Int {{\n        {body}\n    }}\n}}\nprint(C.same(c: C()))\n"
        );
        eprintln!("BEFORE:\n{before}");
        let (after, _) = convert(&before, "swift", true).await;
        eprintln!("AFTER:\n{after}");
        assert!(after.contains("let c: C = self;"), "{after}");
        assert!(after.contains(body), "{after}");
        assert!(!after.contains("return print("), "{after}");
        runtime_control(&before, &after);
    }
}

fn runtime_control(before: &str, after: &str) {
    if available("swiftc") {
        assert_eq!(
            compiled("swiftc", "swift", after),
            compiled("swiftc", "swift", before)
        );
    } else {
        eprintln!(
            "Swift compiler unavailable; public API source controls only; no Swift runtime proof"
        );
    }
}
