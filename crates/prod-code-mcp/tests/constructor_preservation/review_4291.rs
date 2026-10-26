/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::review_4291_support::{check, check_factory, checks};
use futures_util::FutureExt;
use std::panic::AssertUnwindSafe;

#[tokio::test]
async fn python_matched_declaration_bounds() {
    check("python-bounds", "py", "Config", "class BaseConfig:\n    pass\n\nclass Config(BaseConfig):\n    def __init__(self, name):\n        self.name = name\nc = Config('Ada')\nprint(c.name)\n", 1).await;
}

#[tokio::test]
async fn python_direct_constructor_only() {
    check("python-nested-init", "py", "Config", "class Config:\n    class Options:\n        def __init__(self, verbose=False):\n            self.verbose = verbose\n    def __init__(self, name, age=3):\n        self.name = name\n        self.age = age\nc = Config('Ada')\nprint(c.name, c.age)\n", 1).await;
}

#[tokio::test]
async fn python_dataclass_defaults_both_headers() {
    let mut cases = Vec::new();
    for (id, header) in [("plain", ""), ("base", "(object)")] {
        let source = format!(
            "from dataclasses import dataclass, field\ncalls = []\ndef fresh():\n    calls.append(1)\n    return []\n@dataclass\nclass Person{header}:\n    name: str = 'Ada'\n    age: int = 0\n    items: list = field(default_factory=fresh)\na = Person(age=3)\nb = Person(None)\na.items.append(7)\nprint(a.name, a.age, b.name, b.age, a.items, b.items, len(calls))\n"
        );
        cases.push((format!("python-dataclass-{id}"), source));
    }
    checks(
        &cases
            .iter()
            .map(|(id, source)| (id.as_str(), "py", "Person", source.as_str(), 2))
            .collect::<Vec<_>>(),
    )
    .await;
}

#[tokio::test]
async fn typescript_calling_conventions() {
    checks(&[
        ("rest", "export class Bag {\n constructor(public first: number, ...rest: number[]) { this.items = rest; }\n items: number[];\n}\nconst b = new Bag(1, 2, 3);\nconsole.log(b.first, JSON.stringify(b.items));\n", 1),
        ("optional", "export class Bag {\n constructor(public first: number, public label?: string) {}\n}\nconst a = new Bag(1);\nconst b = new Bag(2, 'x');\nconsole.log(a.first, a.label, b.label);\n", 2),
        ("untyped", "export class Bag {\n value: any;\n constructor(value) { this.value = value; }\n}\nconst b = new Bag(7);\nconsole.log(b.value);\n", 1),
        // The class default sees its static member and the earlier constructor
        // argument. It runs once per missing/undefined argument, never at factory
        // definition time or in the new static method's scope.
        ("default-scope", "let calls = 0;\nexport class Bag {\n static seed = 4;\n constructor(public first: number, public value: number = first + Bag.seed + ++calls) { this.argc = arguments.length; }\n argc: number;\n}\nconst a = new Bag(1);\nconst b = new Bag(2, undefined);\nconst c = new Bag(3, 9);\nconsole.log(a.value, b.value, c.value, calls, a.argc, b.argc, c.argc);\n", 3),
    ].iter().map(|&(id, source, count)| (id, "ts", "Bag", source, count)).collect::<Vec<_>>()).await;
}

#[tokio::test]
async fn expression_argument_boundaries() {
    checks(&[
        ("js-comparison", "mjs", "Range", "export class Range {\n constructor(start, end = 9) { this.start = start; this.end = end; }\n}\nconst a = 1, b = 2, c = 5;\nconst r = new Range(a < b, c);\nconst s = new Range(a > b, `a,b${c}`);\nconsole.log(r.start, r.end, s.start, s.end);\n", 2),
        ("py-comparison", "py", "Range", "class Range:\n    def __init__(self, start, end=9):\n        self.start = start\n        self.end = end\na, b, c = 1, 2, 5\nr = Range(a < b, c)\ns = Range(a > b, 'a,b')\nprint(r.start, r.end, s.start, s.end)\n", 2),
    ]).await;
}

#[tokio::test]
async fn rust_proven_enum_variants_are_unrelated() {
    check("rust-enum", "rs", "Config", "struct Config { v: i32 }\nenum Kind { Config { a: i32 } }\nimpl Kind { fn make() -> Self { Self::Config { a: 3 } } }\nfn main() {\n let c = Config { v: 1 };\n let k = Kind::Config { a: 2 };\n let Kind::Config { a } = k;\n let Kind::Config { a: b } = Kind::make();\n println!(\"{} {} {}\", c.v, a, b);\n}\n", 1).await;
}

#[tokio::test]
async fn cpp_copy_list_initialization() {
    check("cpp-copy-list", "cpp", "Point", "#include <iostream>\nstruct Point {\n int x;\n int y;\n};\nint main() {\n Point p = {1, 2};\n const Point q = {3, 4};\n auto r = Point{5, 6};\n std::cout << p.x << q.y << r.x << '\\n';\n}\n", 3).await;
}

#[tokio::test]
async fn python_blank_lines_before_direct_members() {
    checks(&[
        ("python-blank-init", "py", "Config", "class Config:\n\n    \n    # first member follows blank lines\n    def __init__(self, value: int):\n        self.value = value\nc = Config(7)\nprint(c.value)\n", 1),
        ("python-blank-annotations", "py", "Config", "from dataclasses import dataclass\n@dataclass\nclass Config:\n\n    \n    value: int = 7\nc = Config()\nd = Config(9)\nprint(c.value, d.value)\n", 2),
    ]).await;
}

#[tokio::test]
async fn cpp_partial_copy_list_initialization() {
    check("cpp-partial-copy-list", "cpp", "Point", "#include <iostream>\nstruct Point {\n int x;\n int y;\n};\nint main() {\n Point p = {1};\n Point q = {};\n const Point r = {3, 4};\n std::cout << p.x << p.y << q.x << q.y << r.x << r.y << '\\n';\n}\n", 3).await;
}

#[tokio::test]
async fn typescript_nonpublic_constructor_factories() {
    let mut failed = Vec::new();
    for visibility in ["private", "protected"] {
        let source = format!(
            "export class Config {{\n {visibility} constructor(public value: number = 1, public label?: string) {{}}\n static make() {{ return new Config(2); }}\n}}\nconsole.log(Config.make().value);\n"
        );
        if AssertUnwindSafe(check_factory(
            &format!("ts-{visibility}"),
            "ts",
            "Config",
            &source,
            1,
        ))
        .catch_unwind()
        .await
        .is_err()
        {
            failed.push(visibility);
        }
    }
    assert!(failed.is_empty(), "failed visibilities: {failed:?}");
}
