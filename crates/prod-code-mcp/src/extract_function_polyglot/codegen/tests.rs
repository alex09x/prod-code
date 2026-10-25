/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::lang::Language::{self, *};
use super::super::types::{ExtractedParam, OutputKind};
use super::{generate_call_replacement, generate_function_code};

fn call(lang: Language, is_method: bool, output: &OutputKind) -> String {
    generate_call_replacement(
        "calc",
        &["a".to_string(), "b".to_string()],
        &["x".to_string(), "y".to_string()],
        output,
        lang,
        is_method,
        "  ",
    )
}

#[test]
fn call_shape_by_language_and_receiver() {
    let rows: &[(Language, bool, &str)] = &[
        (Python, false, "  calc(a, b)"),
        (Python, true, "  self.calc(a, b)"),
        (TypeScript, true, "  this.calc(a, b);"),
        (JavaScript, true, "  this.calc(a, b);"),
        (Java, true, "  this.calc(a, b);"),
        (Csharp, true, "  this.calc(a, b);"),
        (Kotlin, true, "  calc(a, b)"),
        (Cpp, true, "  this->calc(a, b);"),
        (C, true, "  this->calc(a, b);"),
        (Go, true, "  r.calc(a, b)"),
        (Rust, false, "  calc(a, b);"),
        (Rust, true, "  self.calc(a, b);"),
        (Zig, true, "  self.calc(a, b);"),
        (Swift, false, "  calc(x: a, y: b)"),
        (Swift, true, "  self.calc(x: a, y: b)"),
    ];
    for (lang, is_method, expected) in rows {
        assert_eq!(
            call(*lang, *is_method, &OutputKind::Void),
            *expected,
            "{lang:?} method={is_method}"
        );
    }
}

#[test]
fn call_shape_by_output_kind() {
    // expression, return, new variable, existing variable, several variables
    let rows: &[(Language, [&str; 5])] = &[
        (
            Python,
            [
                "  calc(a, b)",
                "  return calc(a, b)",
                "  r = calc(a, b)",
                "  r = calc(a, b)",
                "  p, q = calc(a, b)",
            ],
        ),
        (
            TypeScript,
            [
                "  calc(a, b)",
                "  return calc(a, b);",
                "  const r = calc(a, b);",
                "  r = calc(a, b);",
                "  const [p, q] = calc(a, b);",
            ],
        ),
        (
            JavaScript,
            [
                "  calc(a, b)",
                "  return calc(a, b);",
                "  const r = calc(a, b);",
                "  r = calc(a, b);",
                "  const [p, q] = calc(a, b);",
            ],
        ),
        (
            Go,
            [
                "  calc(a, b)",
                "  return calc(a, b)",
                "  r := calc(a, b)",
                "  r = calc(a, b)",
                "  p, q := calc(a, b)",
            ],
        ),
        (
            Cpp,
            [
                "  calc(a, b)",
                "  return calc(a, b);",
                "  auto r = calc(a, b);",
                "  r = calc(a, b);",
                "  auto [p, q] = calc(a, b);",
            ],
        ),
        (
            C,
            [
                "  calc(a, b)",
                "  return calc(a, b);",
                "  auto r = calc(a, b);",
                "  r = calc(a, b);",
                "  auto [p, q] = calc(a, b);",
            ],
        ),
        (
            Swift,
            [
                "  calc(x: a, y: b)",
                "  return calc(x: a, y: b)",
                "  let r = calc(x: a, y: b)",
                "  r = calc(x: a, y: b)",
                "  let (p, q) = calc(x: a, y: b)",
            ],
        ),
        (
            Rust,
            [
                "  calc(a, b)",
                "  return calc(a, b);",
                "  let r = calc(a, b);",
                "  r = calc(a, b);",
                "  let (p, q) = calc(a, b);",
            ],
        ),
        (
            Java,
            [
                "  calc(a, b)",
                "  return calc(a, b);",
                "  var r = calc(a, b);",
                "  r = calc(a, b);",
                "  var res = calc(a, b);",
            ],
        ),
        (
            Csharp,
            [
                "  calc(a, b)",
                "  return calc(a, b);",
                "  var r = calc(a, b);",
                "  r = calc(a, b);",
                "  var (p, q) = calc(a, b);",
            ],
        ),
        (
            Kotlin,
            [
                "  calc(a, b)",
                "  return calc(a, b)",
                "  val r = calc(a, b)",
                "  r = calc(a, b)",
                "  val (p, q) = calc(a, b)",
            ],
        ),
        (
            Zig,
            [
                "  calc(a, b)",
                "  return calc(a, b);",
                "  const r = calc(a, b);",
                "  r = calc(a, b);",
                "  const p, q = calc(a, b);",
            ],
        ),
    ];
    let kinds = [
        OutputKind::Expression("a + b".into()),
        OutputKind::EndsWithReturn,
        OutputKind::SingleVar {
            name: "r".into(),
            is_new: true,
        },
        OutputKind::SingleVar {
            name: "r".into(),
            is_new: false,
        },
        OutputKind::MultipleVars(vec!["p".into(), "q".into()]),
    ];
    for (lang, expected) in rows {
        for (kind, want) in kinds.iter().zip(expected) {
            assert_eq!(call(*lang, false, kind), *want, "{lang:?} {kind:?}");
        }
    }
}

fn param(name: &str, ty: Option<&str>) -> ExtractedParam {
    ExtractedParam {
        name: name.into(),
        ty: ty.map(String::from),
    }
}

struct Gen<'a> {
    lang: Language,
    params: Vec<ExtractedParam>,
    output: OutputKind,
    body: &'a str,
    method: Option<&'a str>,
    exported: bool,
    ret: Option<&'a str>,
}

impl Gen<'_> {
    fn run(&self) -> String {
        generate_function_code(
            "f",
            &self.params,
            &self.output,
            self.body,
            self.lang,
            self.method.is_some(),
            self.exported,
            self.method.unwrap_or(""),
            self.ret,
        )
    }
}

fn mk<'a>(lang: Language, output: OutputKind, body: &'a str) -> Gen<'a> {
    Gen {
        lang,
        params: vec![param("a", Some("T")), param("b", None)],
        output,
        body,
        method: None,
        exported: false,
        ret: None,
    }
}

fn expr() -> OutputKind {
    OutputKind::Expression("a + b".into())
}

fn single(name: &str) -> OutputKind {
    OutputKind::SingleVar {
        name: name.into(),
        is_new: true,
    }
}

fn multi() -> OutputKind {
    OutputKind::MultipleVars(vec!["p".into(), "q".into()])
}

#[test]
fn function_bodies_by_output_kind() {
    let rows: &[(Language, OutputKind, &str, &str)] = &[
        (
            Python,
            OutputKind::Void,
            "x = a\nprint(x)\n",
            "def f(a, b):\n    x = a\n    print(x)\n",
        ),
        (Python, expr(), "a + b", "def f(a, b):\n    return a + b\n"),
        (
            Python,
            OutputKind::EndsWithReturn,
            "if a:\n    return b\nreturn a\n",
            "def f(a, b):\n    if a:\n        return b\n    return a\n",
        ),
        (
            Python,
            single("r"),
            "r = a * 2",
            "def f(a, b):\n    r = a * 2\n    return r\n",
        ),
        (
            Python,
            single("r"),
            "r = a * 2\nreturn r",
            "def f(a, b):\n    r = a * 2\n    return r\n",
        ),
        (
            Python,
            multi(),
            "p = a\nq = b",
            "def f(a, b):\n    p = a\n    q = b\n    return p, q\n",
        ),
        (
            TypeScript,
            single("r"),
            "let r = a * 2;",
            "function f(a: T, b: any): number {\n    let r = a * 2;\n    return r;\n}\n",
        ),
        (
            TypeScript,
            single("r"),
            "let r = a * 2;\nreturn r;",
            "function f(a: T, b: any): number {\n    let r = a * 2;\n    return r;\n}\n",
        ),
        (
            TypeScript,
            multi(),
            "const p = a;\nconst q = b;",
            "function f(a: T, b: any) {\n    const p = a;\n    const q = b;\n    return [p, q];\n}\n",
        ),
        (
            TypeScript,
            OutputKind::Void,
            "log(a);",
            "function f(a: T, b: any): void {\n    log(a);\n}\n",
        ),
        (
            TypeScript,
            OutputKind::EndsWithReturn,
            "return a;",
            "function f(a: T, b: any) {\n    return a;\n}\n",
        ),
        (
            JavaScript,
            expr(),
            "",
            "function f(a, b) {\n    return a + b;\n}\n",
        ),
        (
            Go,
            expr(),
            "",
            "func f(a T, b int) int {\n\treturn a + b\n}\n",
        ),
        (
            Go,
            single("r"),
            "r := a",
            "func f(a T, b int) int {\n\tr := a\n\treturn r\n}\n",
        ),
        (
            Go,
            multi(),
            "p := a\nq := b",
            "func f(a T, b int) {\n\tp := a\n\tq := b\n\treturn p, q\n}\n",
        ),
        (
            Go,
            OutputKind::Void,
            "log(a)",
            "func f(a T, b int) {\n\tlog(a)\n}\n",
        ),
        (
            Cpp,
            expr(),
            "",
            "static int f(T a, int b) {\n    return a + b;\n}\n",
        ),
        (
            C,
            OutputKind::Void,
            "log(a);",
            "static void f(T a, int b) {\n    log(a);\n}\n",
        ),
        (
            Cpp,
            multi(),
            "int p = a;",
            "static void f(T a, int b) {\n    int p = a;\n    return p, q;\n}\n",
        ),
        (
            Swift,
            expr(),
            "",
            "func f(a: T, b: Int) -> Int {\n    return a + b\n}\n",
        ),
        (
            Swift,
            OutputKind::Void,
            "log(a)",
            "func f(a: T, b: Int) {\n    log(a)\n}\n",
        ),
        (
            Rust,
            expr(),
            "",
            "fn f(a: T, b: usize) {\n    return a + b;\n}\n",
        ),
        (
            Rust,
            OutputKind::Void,
            "log(a);",
            "fn f(a: T, b: usize) {\n    log(a);\n}\n",
        ),
        (
            Java,
            OutputKind::Void,
            "log(a);",
            "private static void f(T a, Object b) {\n    log(a);\n}\n",
        ),
        (
            Csharp,
            OutputKind::Void,
            "Log(a);",
            "private static void f(T a, object b)\n{\n    Log(a);\n}\n",
        ),
        (
            Kotlin,
            OutputKind::Void,
            "log(a)",
            "private fun f(a: T, b: Any) {\n    log(a)\n}\n",
        ),
        (
            Kotlin,
            multi(),
            "val p = a",
            "private fun f(a: T, b: Any) {\n    val p = a\n    return p, q\n}\n",
        ),
        (
            Zig,
            OutputKind::Void,
            "log(a);",
            "fn f(a: T, b: anytype) void {\n    log(a);\n}\n",
        ),
        (
            Zig,
            expr(),
            "",
            "fn f(a: T, b: anytype) anytype {\n    return a + b;\n}\n",
        ),
        (
            Zig,
            OutputKind::EndsWithReturn,
            "return a;",
            "fn f(a: T, b: anytype) {\n    return a;\n}\n",
        ),
    ];
    for (lang, output, body, expected) in rows {
        let got = mk(*lang, output.clone(), body).run();
        assert_eq!(got, *expected, "{lang:?} {output:?}");
    }
}

#[test]
fn function_headers_with_return_types_visibility_and_receivers() {
    type Row = (&'static str, Gen<'static>, &'static str);
    let with = |lang, output, method, exported, ret| Gen {
        method,
        exported,
        ret,
        ..mk(lang, output, "body")
    };
    let rows: Vec<Row> = vec![
        (
            "python method",
            with(Python, OutputKind::Void, Some("    "), false, None),
            "    def f(self, a, b):\n        body\n",
        ),
        (
            "ts exported",
            with(TypeScript, expr(), None, true, Some("string")),
            "export function f(a: T, b: any): string {\n    return a + b;\n}\n",
        ),
        (
            "ts exported method",
            with(TypeScript, expr(), Some("  "), true, None),
            "  private f(a: T, b: any): number {\n      return a + b;\n  }\n",
        ),
        (
            "js exported",
            with(JavaScript, OutputKind::Void, None, true, None),
            "export function f(a, b) {\n    body\n}\n",
        ),
        (
            "js method",
            with(JavaScript, OutputKind::Void, Some("  "), true, None),
            "  private f(a, b) {\n      body\n  }\n",
        ),
        (
            "go ret",
            with(Go, expr(), None, false, Some("string")),
            "func f(a T, b int) string {\n\treturn a + b\n}\n",
        ),
        (
            "cpp ret",
            with(Cpp, expr(), None, false, Some("double")),
            "static double f(T a, int b) {\n    return a + b;\n}\n",
        ),
        (
            "cpp method",
            with(Cpp, OutputKind::Void, Some("  "), false, None),
            "  void f(T a, int b) {\n      body\n  }\n",
        ),
        (
            "swift method",
            with(Swift, single("r"), Some("  "), false, Some("Bool")),
            "  private func f(a: T, b: Int) -> Bool {\n      body\n      return r\n  }\n",
        ),
        (
            "java public static",
            with(Java, expr(), None, true, Some("int")),
            "public static int f(T a, Object b) {\n    return a + b;\n}\n",
        ),
        (
            "java method",
            with(Java, OutputKind::Void, Some("  "), false, None),
            "  private void f(T a, Object b) {\n      body\n  }\n",
        ),
        (
            "csharp public method",
            with(Csharp, expr(), Some("  "), true, Some("int")),
            "  public int f(T a, object b)\n  {\n      return a + b;\n  }\n",
        ),
        (
            "kotlin public ret",
            with(Kotlin, expr(), None, true, Some("Int")),
            "fun f(a: T, b: Any): Int {\n    return a + b\n}\n",
        ),
        (
            "kotlin method",
            with(Kotlin, single("r"), Some("  "), false, Some("Int")),
            "  private fun f(a: T, b: Any): Int {\n      body\n      return r\n  }\n",
        ),
        (
            "zig pub ret",
            with(Zig, expr(), None, true, Some("u8")),
            "pub fn f(a: T, b: anytype) u8 {\n    return a + b;\n}\n",
        ),
        (
            "zig method",
            with(Zig, OutputKind::Void, Some("  "), false, None),
            "  fn f(a: T, b: anytype) void {\n      body\n  }\n",
        ),
    ];
    for (label, g, expected) in &rows {
        assert_eq!(g.run(), *expected, "{label}");
    }
}

#[test]
fn java_and_csharp_instance_use_decides_static() {
    for (body, is_static) in [("this.x + a", false), ("this + a", false), ("x + a", true)] {
        let java = Gen {
            ret: Some("int"),
            ..mk(Java, expr(), body)
        }
        .run();
        let cs = Gen {
            ret: Some("int"),
            ..mk(Csharp, expr(), body)
        }
        .run();
        assert_eq!(java.contains("private static int f"), is_static, "{java}");
        assert_eq!(cs.contains("private static int f"), is_static, "{cs}");
        assert!(java.contains("private ") && cs.contains("private "));
    }
}

#[test]
fn reindent_inside_bodies_uses_tabs_when_the_selection_has_tabs() {
    let got = mk(Python, OutputKind::Void, "\tx = a\n\tprint(x)").run();
    assert_eq!(got, "def f(a, b):\n\tx = a\n\tprint(x)\n");
}
