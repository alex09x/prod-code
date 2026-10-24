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
fn coverage_declaration_names() {
    use Language::*;
    for (lang, text, expected) in [
        (Python, " async def work(x):", Some("work")),
        (Python, "def broken", None),
        (Go, "func (r *Record) Work(x int) {", Some("Work")),
        (Go, "func Work(x int) {", Some("Work")),
        (Go, "func (broken", None),
        (Go, "func Work", None),
        (Swift, "public func work(x: Int) {", Some("work")),
        (Swift, "func work<T>", Some("work")),
        (
            TypeScript,
            "export function work(x: number) {",
            Some("work"),
        ),
        (JavaScript, "async work(x) {", Some("work")),
        (TypeScript, "function work<T>", Some("work")),
        (Cpp, "int *Record::work(int x) {", Some("work")),
        (C, "int &work(int x) {", Some("work")),
        (Java, "public int work(int x) {", Some("work")),
        (Rust, "fn work(x: i32) {", None),
        (Python, "pass", None),
        (Swift, "func broken", None),
        (JavaScript, "function broken", None),
        (JavaScript, "if (true) {", None),
        (Cpp, "while (true) {", None),
        (JavaScript, "for (;;) {", None),
        (C, "switch (x) {", None),
        (JavaScript, "x.y()", None),
        (Cpp, "x-y()", None),
        (Java, "", None),
        (JavaScript, "()", None),
    ] {
        assert_eq!(
            extract_decl_name_from_line(text, lang).as_deref(),
            expected,
            "{text}"
        );
    }
}

#[test]
fn coverage_insertion_offsets() {
    for (body, offset, indent) in [
        ("\n    return x\n", 1, "    "),
        ("\n# comment\n\n    return x\n", 12, "    "),
        ("\n    \"\"\"brief\"\"\"\n    return x\n", 21, "    "),
        ("\n    '''brief'''\n    return x\n", 21, "    "),
        (
            "\n    \"\"\"\n    docs\n    \"\"\"\n    return x\n",
            33,
            "    ",
        ),
        ("\n    '''\n    docs\n    '''\n    return x\n", 33, "    "),
        ("", 0, "    "),
    ] {
        let text = format!("def f(x):{body}");
        let colon = text.find(':').unwrap();
        // Offset is measured from the end of the signature.
        let expected_offset = match body {
            b if b.contains("brief") => text.find("    return").unwrap(),
            b if b.contains("docs") => text.find("    return").unwrap(),
            b if b.contains("comment") => text.find("    return").unwrap(),
            _ => colon + 1 + offset,
        };
        assert_eq!(
            python_insertion_offset_and_indent(&text, colon, 0),
            (expected_offset, indent.into()),
            "{body:?}"
        );
    }
    let text = "def f():\n    # note\n    pass\n\ndef g():\n    pass\n";
    assert_eq!(
        find_python_body_close(text, 0, 7),
        text.find("def g").unwrap()
    );
    assert_eq!(find_python_body_close("def f():\n    pass", 0, 7), 17);
    for (text, lang, indent) in [
        ("{}", Language::Go, "\t"),
        ("{\n}", Language::Java, "    "),
        ("{\n  return 1;\n}", Language::TypeScript, "  "),
    ] {
        assert_eq!(
            brace_insertion_offset_and_indent(text, 0, text.len() - 1, lang),
            (1, indent.into())
        );
    }
}

#[test]
fn coverage_binding_variants() {
    use Language::*;
    for (lang, ty, value, expected) in [
        (Cpp, Some("const int x"), "3", "const int x = 3;"),
        (C, None, "3", "const auto x = 3;"),
        (C, Some("x"), "3", "const x x = 3;"),
        (TypeScript, None, "3", "const x = 3;"),
        (Swift, None, "3", "let x = 3"),
        (Rust, Some("i32"), "3", "let x: i32 = 3;"),
        (Java, None, "3", "var x = 3;"),
        (Java, Some("int"), "3", "int x = 3;"),
        (Go, None, "&Item{}", "var x = &Item{}"),
        (Go, None, "Item{}", "var x = Item{}"),
        (Go, None, "nil", "var x = nil"),
    ] {
        assert_eq!(format_binding("x", ty, value, lang), expected);
    }
}

#[test]
fn coverage_find_declaration_and_refusals() {
    let text = "function target(x: number) { return x; }";
    for name in ["target", "Box::target", "Box.target", " target "] {
        let decl = find_polyglot_declaration(text, Language::TypeScript, None, Some(name)).unwrap();
        assert_eq!(
            (
                decl.fn_name.as_str(),
                decl.open_paren,
                decl.close_paren,
                decl.body_open,
                decl.body_close
            ),
            ("target", 15, 25, 27, 39)
        );
        assert_eq!(decl.params.len(), 1);
        assert_eq!(decl.params[0].name, "x");
        assert_eq!(decl.receiver, None);
    }
    let decl = find_polyglot_declaration(text, Language::TypeScript, Some(1), None).unwrap();
    assert_eq!(decl.fn_name, "target");
    let py = "def target(self, x):\n    return x\n";
    let decl = find_polyglot_declaration(py, Language::Python, None, Some("target")).unwrap();
    assert_eq!(
        (
            decl.open_paren,
            decl.close_paren,
            decl.body_open,
            decl.body_close
        ),
        (10, 18, 19, py.len())
    );
    for (text, line, name, error) in [
        (
            "",
            None,
            None,
            "could not determine function name to inline parameter from",
        ),
        (
            text,
            Some(0),
            None,
            "could not determine function name to inline parameter from",
        ),
        (
            text,
            Some(8),
            None,
            "could not determine function name to inline parameter from",
        ),
        (
            "// nothing",
            Some(1),
            None,
            "could not determine function name to inline parameter from",
        ),
        (
            "function target(",
            None,
            Some("target"),
            "could not find declaration for function `target`",
        ),
        (
            "function target()",
            None,
            Some("target"),
            "could not find declaration for function `target`",
        ),
        (
            "function target() {",
            None,
            Some("target"),
            "could not find declaration for function `target`",
        ),
        (
            "function nontarget() {}",
            None,
            Some("target"),
            "could not find declaration for function `target`",
        ),
        (
            "target<T",
            None,
            Some("target"),
            "could not find declaration for function `target`",
        ),
    ] {
        let result = find_polyglot_declaration(text, Language::TypeScript, line, name);
        assert_eq!(result.err().unwrap().to_string(), error, "{text}");
    }
    assert_eq!(
        find_polyglot_declaration("def target()", Language::Python, None, Some("target"))
            .err()
            .unwrap()
            .to_string(),
        "could not find declaration for function `target`"
    );
    assert_eq!(
        find_polyglot_declaration(
            "function target (x) {}",
            Language::JavaScript,
            None,
            Some("target")
        )
        .unwrap()
        .open_paren,
        16
    );
}
