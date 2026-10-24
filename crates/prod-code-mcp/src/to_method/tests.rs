/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::helpers::{path_start, receiver_argument, receiver_for, receiver_of};
use super::polyglot::{to_method_cpp, to_method_go, to_method_py, to_method_swift, to_method_ts};
use super::types::MadeMethod;
use crate::make_static::Language;

#[test]
fn the_receiver_follows_the_parameters_type() {
    let r = |p: &str| receiver_for(p, "Counter");
    assert_eq!(r("c: &mut Counter"), Some(("&mut self".into(), "c".into())));
    assert_eq!(r("c: &Counter"), Some(("&self".into(), "c".into())));
    assert_eq!(r("c: Counter"), Some(("self".into(), "c".into())));
    assert_eq!(r("mut c: Counter"), Some(("mut self".into(), "c".into())));
    assert_eq!(r("c: &'a Self"), Some(("&'a self".into(), "c".into())));
    assert_eq!(
        receiver_for("w: &Wrapper<T>", "Wrapper<T>"),
        Some(("&self".into(), "w".into()))
    );
    assert_eq!(r("c: &Other"), None);
    assert_eq!(r("(a, b): (Counter, u32)"), None);
}

#[test]
fn the_first_argument_loses_its_borrow_and_keeps_its_shape() {
    assert_eq!(receiver_of("&mut c"), "c");
    assert_eq!(receiver_of("&c"), "c");
    assert_eq!(receiver_of("self.inner"), "self.inner");
    assert_eq!(receiver_of("make()"), "make()");
    assert_eq!(receiver_of("&items[0]"), "items[0]");
    assert_eq!(receiver_of("*boxed"), "(*boxed)");
    assert_eq!(receiver_of("a + b"), "(a + b)");
    assert_eq!(
        receiver_argument("cond ? a : b", Language::TypeScript),
        "cond ? a : b"
    );
    assert_eq!(
        receiver_argument("target = value", Language::Cpp),
        "target = value"
    );
    assert_eq!(receiver_argument("label: value", Language::Swift), "value");
}

#[test]
fn the_path_in_front_of_a_call_is_found_whole() {
    let t = "x = crate::m::Counter::bump(&mut c, 1);";
    let at = t.find("bump").unwrap();
    assert_eq!(&t[path_start(t, at)..at], "crate::m::Counter::");
    let t = "Self::peek(c)";
    assert_eq!(path_start(t, 6), 0);
    let t = "peek(c)";
    assert_eq!(path_start(t, 0), 0);
}

#[test]
fn the_report_says_what_became_the_receiver_and_what_stayed() {
    let done = MadeMethod {
        owner: "Counter".into(),
        method: "bump".into(),
        root: "/nonexistent".into(),
        file: "src/lib.rs".into(),
        parameter: "c: &mut Counter".into(),
        receiver: "&mut self".into(),
        renamed_uses: 2,
        rewritten_calls: 1,
        unchanged: vec!["src/lib.rs:9:5 (the function used as a value)".into()],
        unmatched: vec![],
        rewritten: vec![],
        diagnostics: vec![],
        applied: false,
    };
    let text = done.render(1000);
    assert!(
        text.contains("`c: &mut Counter` becomes the receiver `&mut self`"),
        "{text}"
    );
    assert!(text.contains("2 use(s) of it in the body"), "{text}");
    assert!(
        text.contains("left as they are, and still valid (1)"),
        "{text}"
    );
    assert!(text.contains("nothing was written"), "{text}");
}

#[test]
fn to_method_ts_transforms_declaration_and_calls() {
    let ts_code = r#"class Calculator {
    static add(c: Calculator, x: number): number {
        return c.val + x;
    }
}

function test() {
    const calc = new Calculator();
    const res = Calculator.add(calc, 5);
}
"#;
    let (owner, method, param, recv, transformed, renamed, calls) =
        to_method_ts(ts_code, Some("Calculator"), "add").unwrap();
    assert_eq!(owner, "Calculator");
    assert_eq!(method, "add");
    assert_eq!(param, "c: Calculator");
    assert_eq!(recv, "this");
    assert_eq!(renamed, 1);
    assert_eq!(calls, 1);
    assert!(transformed.contains("add(x: number): number {"));
    assert!(transformed.contains("return this.val + x;"));
    assert!(transformed.contains("calc.add(5)"));
}

#[test]
fn to_method_py_transforms_declaration_and_calls() {
    let py_code = r#"class MathUtil:
    @staticmethod
    def multiply(u: MathUtil, y: int) -> int:
        return u.factor * y

def run():
    util = MathUtil()
    result = MathUtil.multiply(util, 6)
"#;
    let (owner, method, param, recv, transformed, renamed, calls) =
        to_method_py(py_code, Some("MathUtil"), "multiply").unwrap();
    assert_eq!(owner, "MathUtil");
    assert_eq!(method, "multiply");
    assert_eq!(param, "u: MathUtil");
    assert_eq!(recv, "self");
    assert_eq!(renamed, 1);
    assert_eq!(calls, 1);
    assert!(!transformed.contains("@staticmethod"));
    assert!(transformed.contains("def multiply(self, y: int) -> int:"));
    assert!(transformed.contains("return self.factor * y"));
    assert!(transformed.contains("util.multiply(6)"));
}

#[test]
fn to_method_cpp_transforms_declaration_and_calls() {
    let cpp_code = r#"class Counter {
public:
    static int increment(Counter& c, int step) {
        return c.val + step;
    }
};

void run() {
    Counter cnt;
    int res = Counter::increment(cnt, 2);
}
"#;
    let (owner, method, param, recv, transformed, renamed, calls) =
        to_method_cpp(cpp_code, Some("Counter"), "increment").unwrap();
    assert_eq!(owner, "Counter");
    assert_eq!(method, "increment");
    assert_eq!(param, "Counter& c");
    assert_eq!(recv, "*this");
    assert_eq!(renamed, 1);
    assert_eq!(calls, 1);
    assert!(transformed.contains("int increment(int step) {"));
    assert!(transformed.contains("return this->val + step;"));
    assert!(transformed.contains("cnt.increment(2)"));
}

#[test]
fn to_method_swift_transforms_declaration_and_calls() {
    let swift_code = r#"class Greeter {
    static func greet(g: Greeter, name: String) -> String {
        return g.prefix + name
    }
}

func test() {
    let grt = Greeter()
    let msg = Greeter.greet(grt, name: "Alice")
}
"#;
    let (owner, method, param, recv, transformed, renamed, calls) =
        to_method_swift(swift_code, Some("Greeter"), "greet").unwrap();
    assert_eq!(owner, "Greeter");
    assert_eq!(method, "greet");
    assert_eq!(param, "g: Greeter");
    assert_eq!(recv, "self");
    assert_eq!(renamed, 1);
    assert_eq!(calls, 1);
    assert!(transformed.contains("func greet(name: String) -> String {"));
    assert!(transformed.contains("return self.prefix + name"));
    assert!(transformed.contains("grt.greet(name: \"Alice\")"));
}

#[test]
fn to_method_go_transforms_declaration_and_calls() {
    let go_code = r#"package main

type Service struct {
    tag string
}

func Process(s *Service, data string) string {
    return s.tag + ":" + data
}

func main() {
    svc := &Service{tag: "svc"}
    res := Process(svc, "test")
}
"#;
    let (owner, method, param, recv, transformed, renamed, calls) =
        to_method_go(go_code, Some("Service"), "Process").unwrap();
    assert_eq!(owner, "Service");
    assert_eq!(method, "Process");
    assert_eq!(param, "s *Service");
    assert_eq!(recv, "(s *Service)");
    assert_eq!(renamed, 0);
    assert_eq!(calls, 1);
    assert!(transformed.contains("func (s *Service) Process(data string) string {"));
    assert!(transformed.contains("svc.Process(\"test\")"));
}

fn made(file: &str, owner: &str, receiver: &str) -> MadeMethod {
    MadeMethod {
        owner: owner.into(),
        method: "m".into(),
        root: "/nonexistent".into(),
        file: file.into(),
        parameter: "p: T".into(),
        receiver: receiver.into(),
        renamed_uses: 3,
        rewritten_calls: 4,
        unchanged: vec![],
        unmatched: vec![],
        rewritten: vec![],
        diagnostics: vec![],
        applied: false,
    }
}

#[test]
fn the_report_spells_the_call_target_and_receiver_per_language() {
    // (file, owner, receiver, call target, receiver spelled in the body)
    let cases = [
        ("a.ts", "C", "this", "C.m", "this"),
        ("a.tsx", "C", "this", "C.m", "this"),
        ("a.js", "C", "this", "C.m", "this"),
        ("a.jsx", "C", "this", "C.m", "this"),
        ("a.py", "C", "self", "C.m", "self"),
        ("a.swift", "C", "self", "C.m", "self"),
        ("a.go", "S", "(s *S)", "S.m", "(s *S)"),
        ("a.go", "", "(s *S)", "m", "(s *S)"),
        ("a.cpp", "C", "*this", "C::m", "this"),
        ("a.cc", "C", "*this", "C::m", "this"),
        ("a.cxx", "C", "*this", "C::m", "this"),
        ("a.h", "C", "*this", "C::m", "this"),
        ("a.hpp", "C", "*this", "C::m", "this"),
        ("a.rs", "C", "&self", "C::m", "self"),
        ("a.rs", "", "&self", "m", "self"),
    ];
    for (file, owner, receiver, target, in_body) in cases {
        let text = made(file, owner, receiver).render(100);
        let want = format!(
            "`{target}` ({file})\n\n- the parameter `p: T` becomes the receiver `{receiver}`; 3 use(s) of it in the body are now `{in_body}`\n- 4 call site(s) now call it as a method\n\n0 changed line(s) in 0 file(s)\n\n\nthe analyzer accepts the result: 0 errors\n\nnothing was written; pass `apply: true` to make these edits\n"
        );
        assert_eq!(text, want, "{file} {owner:?}");
    }
}

#[test]
fn the_report_shows_the_diff_cut_to_budget_unmatched_positions_and_rejections() {
    let dir = tempfile::Builder::new().prefix("tm").tempdir().unwrap();
    let path = dir.path().join("lib.rs");
    std::fs::write(&path, "fn a() {}\nfn b() {}\n").unwrap();
    let mut done = made("lib.rs", "C", "&self");
    done.root = dir.path().to_path_buf();
    done.rewritten = vec![(
        path.to_string_lossy().into_owned(),
        "fn a() {}\nfn c() {}\n".into(),
    )];
    done.unmatched = vec!["lib.rs:7:1".into()];
    done.diagnostics = vec!["lib.rs:2: nope".into()];
    done.applied = true;
    let full = done.render(10_000);
    assert!(full.contains("2 changed line(s) in 1 file(s)\n\n--- a/lib.rs\n+++ b/lib.rs\n@@ -1,2 +1,2 @@\n fn a() {}\n-fn b() {}\n+fn c() {}\n"));
    assert!(full.contains(
        "\nnot rewritten; nothing is written while any remains, forced or not:\n  lib.rs:7:1\n"
    ));
    assert!(full.contains("\nthe analyzer rejects the result:\n  lib.rs:2: nope\n"));
    assert!(full.ends_with("\n[applied to 1 file(s)]\n"));
    let cut = done.render(5);
    assert!(cut.contains("file(s)\n\n--- a\n… diff truncated\n"));
}

#[tokio::test]
async fn a_polyglot_request_that_cannot_be_planned_fails_before_the_analyzer_is_asked() {
    use super::polyglot::convert_to_method_polyglot;
    let dir = tempfile::Builder::new().prefix("tm").tempdir().unwrap();
    let root = dir.path();
    let addr: std::net::SocketAddr = "127.0.0.1:9".parse().unwrap();
    let run = |file: std::path::PathBuf| async move {
        convert_to_method_polyglot(addr, root, &file, Some("C"), "nope", false, false)
            .await
            .unwrap_err()
            .to_string()
    };
    let gone = root.join("gone.ts");
    assert_eq!(
        run(gone.clone()).await,
        format!("Cannot read {}", gone.display())
    );
    let notes = root.join("notes.txt");
    std::fs::write(&notes, "class C {}\n").unwrap();
    assert_eq!(
        run(notes.clone()).await,
        format!("Unsupported language for file: {}", notes.display())
    );
    let mut seen = Vec::new();
    for name in ["a.ts", "a.py", "a.cpp", "a.swift", "a.go"] {
        let file = root.join(name);
        std::fs::write(&file, "").unwrap();
        seen.push(format!("{name}: {}", run(file).await));
    }
    assert_eq!(
        seen,
        [
            "a.ts: Could not find class in TypeScript/JavaScript file",
            "a.py: Could not find class in Python file",
            "a.cpp: Could not find class/struct in C++ file",
            "a.swift: Could not find class/struct in Swift file",
            "a.go: Function `nope` not found in Go file",
        ]
    );
}
