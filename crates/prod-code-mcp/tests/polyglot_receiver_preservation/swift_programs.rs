/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::support::convert;
use prod_code_mcp::to_method::convert_to_method_polyglot;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};

async fn program(name: &str, before: &str, auxiliary: &str, expected: &str) -> String {
    let after = if auxiliary.is_empty() {
        convert(before, "swift", true).await.0
    } else {
        let ws = Workspace::new(&[("main.swift", before), ("base.swift", auxiliary)]);
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
            false,
        )
        .await
        .unwrap();
        let disk = std::fs::read_to_string(path).unwrap();
        assert_eq!(disk, result.rewritten[0].1);
        disk
    };
    eprintln!(
        "SWIFT_PROGRAM {}",
        serde_json::json!({
            "name": name, "before": before, "after": after,
            "auxiliary": auxiliary, "expected": expected,
            "analyzer": "scripted empty diagnostics; compiler/runtime checked separately"
        })
    );
    after
}

#[tokio::test]
async fn review18_swift_complete_receiver_programs() {
    for (name, body) in [
        ("direct", "return c === other"),
        ("implicit_direct", "c === other"),
    ] {
        let source = format!(
            "class C {{\n    static func same(_ c: C, other: C) -> Bool {{ {body} }}\n}}\nlet value = C()\nprint(C.same(value, other: value))\n"
        );
        let after = program(name, &source, "", "true\n").await;
        assert!(after.contains("let c: C = self;"));
    }
    let source = "class C {\n    var value = 3\n    subscript(index: Int) -> Int { value }\n    static func same(c: C, other: C) -> Int {\n        let take = { (value: C) in value.value }\n        let capture = { [c] in c.value }\n        let shadow = { (c: C) in c.value }\n        return take(c) + capture() + shadow(other) + c[0]\n    }\n}\nlet value = C()\nprint(C.same(c: value, other: value))\n";
    program("index_argument_capture", source, "", "12\n").await;
    let source = "class C {\n    var value = 7\n    static func same(c: C) -> Int { return c.value }\n}\nprint(C.same(c: C()))\n";
    let after = program("property", source, "", "7\n").await;
    assert!(after.contains("return self.value"));
}

#[tokio::test]
async fn review18_swift_complete_alias_inherited_programs() {
    let source = "class C {\n    var value = 7\n    static func same(c: C) -> Void { c.value }\n}\nenum Names {}\ntypealias NamesAlias = Names\nextension NamesAlias { typealias Alias = C }\nextension NamesAlias.Alias { typealias Void = Int }\nprint(C.same(c: C()))\n";
    let after = program("alias_result", source, "", "7\n").await;
    assert!(after.contains("return c.value"));
    let source = "class Base { typealias Void = Int }\ntypealias Parent = Base\nclass C: Parent {\n    var value = 7\n    static func same(c: C) -> Void { c.value }\n}\nprint(C.same(c: C()))\n";
    program("inherited_result", source, "", "7\n").await;
    let source = "class C: Base {\n    var value = 7\n    static func same(c: C) -> Void {\n        print(\"c.value\".wholeMatch(of: #/c.value/#) != nil)\n        print(c.value)\n    }\n}\nC.same(c: C())\n";
    let after = program(
        "external_unit_regex",
        source,
        "class Base {}\n",
        "true\n7\n",
    )
    .await;
    assert!(!after.contains("return print("));
    let source = "class C: Base {\n    static func same(c: C) -> Void { #/c.value/# }\n}\nprint(\"c.value\".wholeMatch(of: C.same(c: C())) != nil)\n";
    program(
        "external_inherited_regex",
        source,
        "class Base { typealias Void = Regex<Substring> }\n",
        "true\n",
    )
    .await;
}

#[tokio::test]
async fn review18_swift_complete_regex_programs() {
    for (name, regex) in [
        ("extended_regex", "#/c.value/#"),
        ("bare_regex", "/c.value/"),
    ] {
        let source = format!(
            "class C {{\n    static func same(c: C) -> Regex<Substring> {{ return {regex} }}\n}}\nlet pattern = #/C.same(c: C())/#\nprint(\"c.value\".wholeMatch(of: C.same(c: C())) != nil)\nprint(\"C.same(c: C())\".wholeMatch(of: pattern) != nil)\n"
        );
        let after = program(name, &source, "", "true\nfalse\n").await;
        assert!(after.contains(&format!("return {regex}")), "{after}");
        assert!(after.contains("let pattern = #/C.same(c: C())/#"));
    }
    let source = "class C {\n    let pattern = #/\nclass C {1}\nfunc same(c: C)\n\\}\n/#\n    var value = \"c.value}\"\n    static func same(c: C) -> Bool { return c.value.wholeMatch(of: #/c.value\\}/#) != nil }\n}\nprint(C.same(c: C()))\n";
    let after = program("regex_boundaries", source, "", "true\n").await;
    assert!(after.contains("let c: C = self; return c.value.wholeMatch"));
    assert!(after.contains("class C {1}\nfunc same(c: C)\n\\}"));
    let source = "class C {\n    var value = \"c.value,foo(bar)\"\n    static func same(c: C, regex: Regex<Substring> = #/c.value,foo\\(bar\\)/#) -> Bool { return c.value.wholeMatch(of: regex) != nil }\n}\nprint(C.same(c: C(), regex: #/c.value,foo\\(bar\\)/#))\n";
    let after = program("regex_arguments", source, "", "true\n").await;
    assert_eq!(
        after.matches("#/c.value,foo\\(bar\\)/#").count(),
        2,
        "{after}"
    );
    let source = "class C {\n    var value = 7\n    static func same(c: C) -> String { return \"\\(c.value)-\\(\"c.value\".wholeMatch(of: #/c.value/#) != nil)\" }\n}\nprint(\"\\(C.same(c: C()))\")\n";
    let after = program("string_interpolation_regex", source, "", "7-true\n").await;
    assert!(after.contains("print(\"\\(C().same())\")"), "{after}");
    let source = "class C {\n    static func same(c: C) -> Regex<Substring> { #/c.value\"return\";let/# }\n}\nprint(\"c.value\\\"return\\\";let\".wholeMatch(of: C.same(c: C())) != nil)\n";
    program("regex_quotes_keywords", source, "", "true\n").await;
}

#[tokio::test]
async fn review18_swift_bare_division_and_interpolation_controls() {
    let source = "func / (lhs: String, rhs: Int) -> Int { lhs.count / rhs }\nclass C {\n    var value = 2\n    static func same(c: C) -> Int { return \"abcdefgh\"/c.value/2 }\n}\nprint(C.same(c: C()))\n";
    let after = program("division_after_string", source, "", "2\n").await;
    assert!(after.contains("/self.value/2"), "{after}");
    let source = "class C {\n    static func same(c: C) -> Regex<Substring> { return /* \" ignored */ /c.value/ }\n}\nprint(\"c.value\".wholeMatch(of: C.same(c: C())) != nil)\n";
    let after = program("bare_regex_after_comment", source, "", "true\n").await;
    assert!(after.contains("/* \" ignored */ /c.value/"), "{after}");
    let source = "class C {\n    var value = 7\n    static func same(c: C) -> Regex<Substring> { return #/\\#(c.value)/# }\n}\nprint(C.same(c: C()))\n";
    let (after, _) = convert(source, "swift", true).await;
    assert!(after.contains("#/\\#(c.value)/#"));
    eprintln!(
        "SWIFT_UNSUPPORTED {}",
        serde_json::json!({"name": "regex_interpolation", "before": source, "after": after})
    );
}

#[tokio::test]
async fn review18_swift_ternary_regex_control() {
    let source = "class C {\n    static func same(c: C) -> Regex<Substring> { return true ? /c.value/ : /c.other/ }\n}\nprint(\"c.value\".wholeMatch(of: C.same(c: C())) != nil)\n";
    let after = program("ternary_regex", source, "", "true\n").await;
    assert!(after.contains("true ? /c.value/ : /c.other/"), "{after}");
}

#[tokio::test]
async fn review18_swift_regex_escape_capture_control() {
    let source = "class C {\n    var value = 7\n    static func same(c: C) -> Regex<(Substring, Substring)> { return #/\\#(c.value)/# }\n}\nprint(\"#c.value\".wholeMatch(of: C.same(c: C())) != nil)\n";
    let after = program("regex_escape_capture", source, "", "true\n").await;
    assert!(after.contains("#/\\#(c.value)/#"), "{after}");
}

#[tokio::test]
async fn review19_swift_slash_operator_is_executable() {
    let source = "infix operator +/ : AdditionPrecedence\nfunc +/ (lhs: Int, rhs: Int) -> Int { lhs + rhs }\nclass C {\n    var value = 4\n    static func same(c: C) -> Int { return 8+/c.value/2 }\n}\nprint(C.same(c: C()))\n";
    let after = program("slash_operator", source, "", "10\n").await;
    assert!(after.contains("return 8+/self.value/2"), "{after}");
}

#[tokio::test]
async fn review19_swift_regex_touches_comment_delimiter() {
    let source = "class C {\n    static func same(c: C) -> Regex<Substring> { return /* ignored *//c.value/ }\n}\nprint(\"c.value\".wholeMatch(of: C.same(c: C())) != nil)\n";
    let after = program("regex_touches_comment", source, "", "true\n").await;
    assert!(after.contains("return /* ignored *//c.value/"), "{after}");
}

#[tokio::test]
async fn review20_swift_prefix_slash_operator_is_executable() {
    for (name, body) in [
        ("prefix_slash_operator", "return /+c.value"),
        (
            "prefix_slash_operator_comment",
            "return /+c.value // c.value\n",
        ),
    ] {
        let source = format!(
            "prefix operator /+\nprefix func /+ (rhs: Int) -> Int {{ rhs + 1 }}\nclass C {{\n    var value = 4\n    static func same(c: C) -> Int {{ {body} }}\n}}\nprint(C.same(c: C()))\n"
        );
        eprintln!("ORIGINAL {name}:\n{source}");
        let after = program(name, &source, "", "5\n").await;
        assert!(after.contains("return /+self.value"), "{after}");
    }
    let source = "class C {\n    static func same(c: C) -> Regex<Substring> { return /^c.value/ }\n}\nprint(\"c.value\".wholeMatch(of: C.same(c: C())) != nil)\n";
    let after = program("operator_head_regex", source, "", "true\n").await;
    assert!(after.contains("return /^c.value/"), "{after}");
}
