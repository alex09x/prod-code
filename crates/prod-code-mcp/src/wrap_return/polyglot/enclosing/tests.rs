/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */
use super::*;

#[test]
fn coverage_enclosing_headers() {
    use Language::*;
    for (lang, source, expected) in [
        (Python, "async def f() -> int:\n    work()", Some(("int", true))),
        (Python, "def f():\n    work()", Some(("None", false))),
        (Python, "def f():\nwork()", None),
        (Python, "work()", None),
        (Swift, "func f() { work() }", Some(("Void", false))),
        (Go, "func (r Item) f(x int) (int, error) { work() }", Some(("(int, error)", false))),
        (Go, "func f() { work() }", Some(("", false))),
        (Cpp, "static int f() { work() }", Some(("static int", false))),
        (C, "f() { work() }", Some(("f", false))),
        (JavaScript, "const f = async () => { work() }", Some(("", true))),
        (TypeScript, "const f = (): number => { work() }", Some(("number", false))),
        (Go, "func f( { work() }", Some(("", false))),
        (Go, "func (r Item) f { work() }", Some(("", false))),
        (Go, "func (r Item f { work() }", Some(("", false))),
        (Go, "func f { work() }", Some(("", false))),
        (Cpp, "namespace { work() }", Some(("", false))),
        (Java, "class Item { work() }", Some(("", false))),
        (Go, "package main\n{ work() }", Some(("", false))),
        (TypeScript, "function f() { work()", None),
    ] {
        let at = source.find("work").unwrap();
        assert_eq!(enclosing_polyglot_info(source, at, lang), expected.map(|(s,b)| (s.into(),b)), "{source}");
    }
    assert_eq!(enclosing_polyglot_info("", 0, Python), None);
}
