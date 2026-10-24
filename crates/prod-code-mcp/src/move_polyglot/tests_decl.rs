/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::parameter_object::Language;

use super::decl::{find_matching_brace_end, find_polyglot_decl, with_doc_comment_polyglot};

use Language::{C, Cpp, Go, Java, JavaScript, Python, Rust, Swift, TypeScript};

#[test]
fn doc_comment_start_table() {
    let cases: &[(Language, &str, u32, u32)] = &[
        (Python, "# c\n@d\ndef f():\n    pass\n", 3, 1),
        (Python, "x\n\n# c\ndef f():\n", 4, 3),
        (Python, "x = 1\ndef f():\n", 2, 2),
        (Python, "def f():\n", 1, 1),
        (Go, "// a\n// b\nfunc F() {}\n", 3, 1),
        (Go, "/* x */\nfunc F() {}\n", 2, 2),
        (Rust, "/// d\n#[test]\nfn f() {}\n", 3, 1),
        (Rust, "//! inner\nfn f() {}\n", 2, 1),
        (Java, "@Override\n// c\nvoid f() {}\n", 3, 1),
        (Cpp, "/* c */\nint f();\n", 2, 1),
        (C, " * mid\nint f();\n", 2, 1),
        (Swift, "/// d\n@objc\nfunc f() {}\n", 3, 1),
        (
            TypeScript,
            "/**\n * Helper\n */\nexport function f() {}\n",
            4,
            1,
        ),
        (JavaScript, "@dec\nclass A {}\n", 2, 1),
        (TypeScript, "const a = 1;\nfunction f() {}\n", 2, 2),
    ];
    for (idx, (lang, text, start, expected)) in cases.iter().enumerate() {
        assert_eq!(
            with_doc_comment_polyglot(text, *start, *lang),
            *expected,
            "case {idx}"
        );
    }
}

#[test]
fn matching_brace_end_table() {
    let cases: &[(&[&str], usize, Option<usize>)] = &[
        (&["fn f() {", "  x;", "}"], 0, Some(2)),
        (&["int f();"], 0, None),
        (&["a { b }"], 0, Some(0)),
        (&["f() { x; }"], 0, Some(0)),
        (&["f() {", " let s = \"}\";", "}"], 0, Some(2)),
        (&["f() {", " c = '}';", "}"], 0, Some(2)),
        (&["f() {", " s = \"\\\"}\";", "}"], 0, Some(2)),
        (&["f() {", " `a", "}`;", "}"], 0, Some(3)),
        (&["f() {", " // }", "}"], 0, Some(2)),
        (&["f() {", " /* }", " } */", "}"], 0, Some(3)),
        (&["f() {", " x"], 0, None),
        (&["a", "b {", "}"], 1, Some(2)),
        (&["x"], 5, None),
    ];
    for (idx, (lines, start, expected)) in cases.iter().enumerate() {
        assert_eq!(
            find_matching_brace_end(lines, *start),
            *expected,
            "case {idx}"
        );
    }
}

#[test]
fn find_decl_ranges() {
    let cases: &[(Language, &str, u32, (&str, u32, u32))] = &[
        (
            Python,
            "class A:\n    x = 1\n\n    def f(self):\n        pass\n\nprint(1)\n",
            1,
            ("A", 1, 6),
        ),
        (
            Python,
            "x = 1\n\n@deco\ndef g():\n    return 1\n",
            2,
            ("g", 3, 5),
        ),
        (
            Python,
            "def f():\n    a = 1\n    return a\n",
            3,
            ("f", 1, 3),
        ),
        (Python, "# c\ndef f():\n    pass", 1, ("f", 1, 3)),
        (
            TypeScript,
            "export type T = number;\nexport const x = 1;\n",
            1,
            ("T", 1, 1),
        ),
        (TypeScript, "const x = [\n  1,\n];\n", 1, ("x", 1, 3)),
        (TypeScript, "class A {\n}\n;\n", 1, ("A", 1, 3)),
        (
            TypeScript,
            "@Component()\nexport class A {\n}\n",
            1,
            ("A", 1, 3),
        ),
        (Cpp, "class A {\n};\nint x;\n", 1, ("A", 1, 2)),
        (Swift, "struct P {\n  var x: Int\n}\n", 1, ("P", 1, 3)),
        (Java, "public class Foo {\n}\n", 1, ("Foo", 1, 2)),
        (Go, "var x = []int{\n\t1,\n}\n", 1, ("x", 1, 3)),
        (Go, "var y = 5\nfunc g() {}\n", 1, ("y", 1, 1)),
        (Go, "var z = 1 +\n\t2\n", 1, ("z", 1, 2)),
        (Go, "var s = `a\n}`\nfunc h() {}\n", 1, ("s", 1, 2)),
        (Go, "var r = '{'\nfunc h() {}\n", 1, ("r", 1, 1)),
        (Go, "var q = \"a\\\"b\"\nfunc h() {}\n", 1, ("q", 1, 1)),
        (Go, "var m = f(\n\t1, // }\n)\n", 1, ("m", 1, 3)),
    ];
    for (idx, (lang, text, line, (name, start, end))) in cases.iter().enumerate() {
        let got = find_polyglot_decl(text, *line, *lang).unwrap();
        assert_eq!(got, (name.to_string(), *start, *end), "case {idx}");
    }
}

#[test]
fn find_decl_refusals() {
    let cases: &[(Language, &str, u32, &str)] = &[
        (Python, "a\n", 0, "line 0 is out of bounds (1..=1)"),
        (Python, "a\n", 5, "line 5 is out of bounds (1..=1)"),
        (
            Python,
            "x = 1\ny = 2\n",
            1,
            "no declaration found at line 1",
        ),
        (
            TypeScript,
            "// c\nfoo();\n",
            1,
            "no declaration found at line 1",
        ),
        (
            Go,
            "func (s *S) Do() {\n}\n",
            1,
            "`Do` is a method with a receiver; move it with `code_move_method`",
        ),
        (
            Go,
            "const (\n\tA = 1\n)\n",
            1,
            "could not parse declaration name at line 1",
        ),
    ];
    for (idx, (lang, text, line, message)) in cases.iter().enumerate() {
        let err = find_polyglot_decl(text, *line, *lang).expect_err("must refuse");
        assert_eq!(err.to_string(), *message, "case {idx}");
    }
}
