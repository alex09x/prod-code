/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::make_static::calls::rewrite_calls_in_code;
use crate::make_static::helpers::{mentions, receiver_has_effects, split_receiver};
use crate::make_static::polyglot::{
    make_static_cpp, make_static_go, make_static_py, make_static_swift, make_static_ts,
};
use crate::make_static::types::{Language, MadeStatic};

fn refs_for(code: &str, method: &str) -> std::collections::HashSet<(u32, u32)> {
    let mut refs = std::collections::HashSet::new();
    for needle in [format!(".{method}("), format!("->{method}(")] {
        for (offset, _) in code.match_indices(&needle) {
            if let Ok(position) =
                crate::signature::position_at(code, offset + needle.find(method).unwrap())
            {
                refs.insert(position);
            }
        }
    }
    refs
}

#[test]
fn a_receiver_is_split_off_the_parameter_list() {
    assert_eq!(
        split_receiver("&self, x: u32"),
        Some(("&self".to_string(), "x: u32".to_string()))
    );
    assert_eq!(
        split_receiver("&mut self"),
        Some(("&mut self".to_string(), String::new()))
    );
    assert_eq!(
        split_receiver("self"),
        Some(("self".to_string(), String::new()))
    );
    assert_eq!(
        split_receiver("mut self, a: u8, b: u8"),
        Some(("mut self".to_string(), "a: u8, b: u8".to_string()))
    );
    assert_eq!(
        split_receiver("&'a self"),
        Some(("&'a self".to_string(), String::new()))
    );
    assert_eq!(
        split_receiver("self: Box<Self>"),
        Some(("self: Box<Self>".to_string(), String::new()))
    );
    assert_eq!(split_receiver("x: u32"), None);
    assert_eq!(split_receiver(""), None);
    assert_eq!(split_receiver("selfish: u8"), None);
}

#[test]
fn only_a_receiver_that_does_something_is_kept() {
    for plain in ["s", "self.store", "crate::GLOBAL", "a.b.c", "&x"] {
        assert!(!receiver_has_effects(plain), "{plain}");
    }
    for effect in [
        "load()",
        "self.get()?",
        "vec![1]",
        "make().await",
        "items[0]",
    ] {
        assert!(receiver_has_effects(effect), "{effect}");
    }
}

#[test]
fn a_word_is_mentioned_only_whole() {
    assert!(mentions("{ self.x }", "self"));
    assert!(!mentions("{ myself }", "self"));
    assert!(!mentions("{ Self::new() }", "self"));
}

fn report() -> MadeStatic {
    MadeStatic {
        owner: "S".into(),
        method: "twice".into(),
        root: "/root".into(),
        file: "src/lib.rs".into(),
        receiver: "&self".into(),
        rewritten_calls: 2,
        blocked: vec!["src/app.rs:3:9 `load()?` is evaluated for what it does".into()],
        unmatched: vec!["src/app.rs:7:5 (not a call: the method used as a value)".into()],
        rewritten: vec![("/root/src/lib.rs".into(), "fn main() {}\n".into())],
        diagnostics: vec!["mismatched types (src/app.rs:4:5)".into()],
        applied: false,
    }
}

#[test]
fn the_report_names_the_receiver_the_calls_and_what_stays() {
    let text = report().render(10_000);
    assert!(text.contains("the receiver `&self` is removed"), "{text}");
    assert!(
        text.contains("2 call site(s) now call `S::twice`"),
        "{text}"
    );
    assert!(text.contains("bind it to a variable first"), "{text}");
    assert!(text.contains("not a call"), "{text}");
    assert!(text.contains("the analyzer rejects the result"), "{text}");
    assert!(text.contains("nothing was written"), "{text}");
    let mut done = report();
    done.blocked.clear();
    done.unmatched.clear();
    done.diagnostics.clear();
    done.applied = true;
    let text = done.render(10);
    assert!(
        text.contains("0 errors")
            && text.contains("[applied to 1 file(s)]")
            && text.contains("diff truncated"),
        "{text}"
    );
}

#[test]
fn make_static_ts_transforms_declaration_and_calls() {
    let ts_code = r#"class Calculator {
    add(a: number, b: number): number {
        return a + b;
    }

    total(): number {
        return this.add(10, 20);
    }
}

function test() {
    const calc = new Calculator();
}
"#;
    let mut refs = refs_for(ts_code, "add");
    let (owner, method, transformed, calls, blocked) =
        make_static_ts(ts_code, Some("Calculator"), "add", Some(&mut refs)).unwrap();
    assert_eq!(owner, "Calculator");
    assert_eq!(method, "add");
    assert_eq!(calls, 1);
    assert!(blocked.is_empty());
    assert!(transformed.contains("static add(a: number, b: number): number {"));
    assert!(transformed.contains("Calculator.add(10, 20)"));
}

#[test]
fn make_static_does_not_rewrite_a_same_named_method_on_an_unresolved_receiver() {
    let code = "const value = set.add(10, 20);\n";
    let mut blocked = Vec::new();
    let (rewritten, count) = rewrite_calls_in_code(
        code,
        "add",
        "Calculator",
        Language::TypeScript,
        "example.ts",
        &mut blocked,
        Some(&mut std::collections::HashSet::new()),
    );
    assert_eq!(count, 0);
    assert_eq!(rewritten, code);
    assert!(blocked.is_empty());
}

#[test]
fn make_static_ts_rejects_this_access() {
    let ts_code = r#"class Counter {
    val: number = 0;
    bump(): void {
        this.val += 1;
    }
}
"#;
    let err = make_static_ts(ts_code, Some("Counter"), "bump", None).unwrap_err();
    assert!(err.to_string().contains("uses `this`"));
}

#[test]
fn make_static_py_transforms_declaration_and_calls() {
    let py_code = r#"class MathUtil:
    def multiply(self, x: int, y: int) -> int:
        return x * y

def run():
    util = MathUtil()
    result = util.multiply(5, 6)
"#;
    let mut refs = refs_for(py_code, "multiply");
    let (owner, method, transformed, calls, blocked) =
        make_static_py(py_code, Some("MathUtil"), "multiply", Some(&mut refs)).unwrap();
    assert_eq!(owner, "MathUtil");
    assert_eq!(method, "multiply");
    assert_eq!(calls, 1);
    assert!(blocked.is_empty());
    assert!(transformed.contains("@staticmethod\n    def multiply(x: int, y: int) -> int:"));
    assert!(transformed.contains("MathUtil.multiply(5, 6)"));
}

#[test]
fn make_static_cpp_transforms_declaration_and_calls() {
    let cpp_code = r#"class Util {
public:
    int sum(int a, int b) const {
        return a + b;
    }
};

void run() {
    Util u;
    int s = u.sum(3, 4);
}
"#;
    let mut refs = refs_for(cpp_code, "sum");
    let (owner, method, transformed, calls, blocked) =
        make_static_cpp(cpp_code, Some("Util"), "sum", Some(&mut refs)).unwrap();
    assert_eq!(owner, "Util");
    assert_eq!(method, "sum");
    assert_eq!(calls, 1);
    assert!(blocked.is_empty());
    assert!(transformed.contains("static int sum(int a, int b) {"));
    assert!(transformed.contains("Util::sum(3, 4)"));
}

#[test]
fn make_static_swift_transforms_declaration_and_calls() {
    let swift_code = r#"class Greeter {
    func greet(name: String) -> String {
        return "Hello " + name
    }
}

func test() {
    let g = Greeter()
    let msg = g.greet(name: "World")
}
"#;
    let mut refs = refs_for(swift_code, "greet");
    let (owner, method, transformed, calls, blocked) =
        make_static_swift(swift_code, Some("Greeter"), "greet", Some(&mut refs)).unwrap();
    assert_eq!(owner, "Greeter");
    assert_eq!(method, "greet");
    assert_eq!(calls, 1);
    assert!(blocked.is_empty());
    assert!(transformed.contains("static func greet(name: String) -> String {"));
    assert!(transformed.contains("Greeter.greet(name: \"World\")"));
}

#[test]
fn make_static_does_not_duplicate_modifier_for_mutating_swift_method() {
    let source = "struct Counter {\n    mutating func reset() {\n    }\n}\n";
    let (_, _, rewritten, _, blocked) = make_static_swift(
        source,
        Some("Counter"),
        "reset",
        Some(&mut std::collections::HashSet::new()),
    )
    .unwrap();
    assert!(rewritten.contains("static func reset()"), "{rewritten}");
    assert!(!rewritten.contains("static static func"), "{rewritten}");
    assert!(blocked.is_empty(), "{blocked:?}");
}

#[test]
fn static_call_rewrite_skips_string_literals_on_lines_with_real_calls() {
    let code = "let example = \"this.add(1)\"; const result = this.add(2);\n";
    let mut blocked = Vec::new();
    let (rewritten, count) = rewrite_calls_in_code(
        code,
        "add",
        "Calculator",
        Language::TypeScript,
        "example.ts",
        &mut blocked,
        Some(&mut refs_for(code, "add")),
    );
    assert_eq!(count, 1);
    assert!(rewritten.contains("\"this.add(1)\""), "{rewritten}");
    assert!(rewritten.contains("Calculator.add(2)"), "{rewritten}");
    assert!(blocked.is_empty(), "{blocked:?}");
}

#[test]
fn make_static_go_transforms_declaration_and_calls() {
    let go_code = r#"package main

type Service struct{}

func (s *Service) Process(data string) string {
    return "processed:" + data
}

func main() {
    svc := &Service{}
    res := svc.Process("test")
}
"#;
    let mut refs = refs_for(go_code, "Process");
    let (owner, method, transformed, calls, blocked) =
        make_static_go(go_code, Some("Service"), "Process", Some(&mut refs)).unwrap();
    assert_eq!(owner, "Service");
    assert_eq!(method, "Process");
    assert_eq!(calls, 1);
    assert!(blocked.is_empty());
    assert!(transformed.contains("func Process(data string) string {"));
    assert!(transformed.contains("Process(\"test\")"));
}

#[test]
fn make_static_catches_effectful_receiver() {
    let ts_code = r#"class Worker {
    run(): void {}
}

function test() {
    getWorker()?.run();
}
"#;
    let mut refs = refs_for(ts_code, "run");
    let (_, _, _, _, blocked) =
        make_static_ts(ts_code, Some("Worker"), "run", Some(&mut refs)).unwrap();
    assert_eq!(blocked.len(), 1);
    assert!(blocked[0].contains("is evaluated for what it does"));
}
