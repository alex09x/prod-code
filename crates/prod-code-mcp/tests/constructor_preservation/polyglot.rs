/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::support::check;
use prod_code_mcp::replace_constructor::ReplaceMode::{Builder, Factory};

#[tokio::test]
async fn javascript_rest_spread() {
    let source = "export class Bag {\n    constructor(...items) { this.items = items; }\n}\nconst values = [1, 2];\nconst bag = new Bag(...values);\nconsole.log(JSON.stringify(bag.items));\n";
    check("javascript-rest", "mjs", "Bag", source, Builder, 1, &[]).await;
}

#[tokio::test]
async fn python_omitted_default() {
    let source = "class Person:\n    def __init__(self, name: str = \"Ada\"):\n        self.name = name\nperson = Person()\nprint(person.name)\n";
    check("python-default", "py", "Person", source, Builder, 1, &[]).await;
}

#[tokio::test]
async fn javascript_spread_fixed_and_rest_positions() {
    for (id, source) in [
        (
            "fixed-spread",
            "export class Bag {\n constructor(a, b = 2) { this.a = a; this.b = b; }\n}\nconst values = [1, 3];\nconst bag = new Bag(...values);\nconsole.log(bag.a, bag.b);\n",
        ),
        (
            "prefix-rest",
            "export class Bag {\n constructor(first, ...items) { this.first = first; this.items = items; }\n}\nconst bag = new Bag(0, 1, 2);\nconsole.log(bag.first, JSON.stringify(bag.items));\n",
        ),
    ] {
        for mode in [Factory, Builder] {
            check(&format!("{id}-{mode}"), "mjs", "Bag", source, mode, 1, &[]).await;
        }
    }
}

#[tokio::test]
async fn python_defaults_explicit_none_and_saved_fields() {
    let source = "class Person:\n    def __init__(self, name: str = \"Ada\", age: int = 3):\n        self.name = name\n        self.age = age\na = Person()\nb = Person(None)\nc = Person(age=7)\nprint(a.name, a.age, b.name, b.age, c.name, c.age)\n";
    for mode in [Factory, Builder] {
        check(
            &format!("python-values-{mode}"),
            "py",
            "Person",
            source,
            mode,
            3,
            &[],
        )
        .await;
    }
}

const CPP: &str = "#include <iostream>\nstruct Point {\n    int x;\n    int y;\n};\nint main() {\n    Point p{1, 2};\n    const Point q{3, 4};\n    auto r = Point{5, 6};\n    std::cout << p.x << q.y << r.x << '\\n';\n}\n";
#[tokio::test]
async fn cpp_declarations_factory() {
    check("cpp-factory", "cpp", "Point", CPP, Factory, 3, &[]).await;
}
#[tokio::test]
async fn cpp_declarations_builder() {
    check("cpp-builder", "cpp", "Point", CPP, Builder, 3, &[]).await;
}

#[tokio::test]
async fn swift_explicit_init() {
    let source = "struct Point {\n    var x: Int\n}\nlet p = Point.init(x: 1)\nlet q = Point(x: 2)\nprint(p.x, q.x)\n";
    for mode in [Factory, Builder] {
        check(
            &format!("swift-{mode}"),
            "swift",
            "Point",
            source,
            mode,
            2,
            &[],
        )
        .await;
    }
}

const GO: &str = "package main\nimport \"fmt\"\ntype Age int\ntype User struct {\n    Name string\n    Age Age\n}\nfunc main() {\n    a := &User{Name: \"Ada\"}\n    b := User{\"x:y\", 37}\n    fmt.Println(a.Name, a.Age, b.Name, b.Age)\n}\n";
#[tokio::test]
async fn go_named_zero_factory() {
    check("go-factory", "go", "User", GO, Factory, 2, &[]).await;
}
#[tokio::test]
async fn go_named_zero_builder() {
    check("go-builder", "go", "User", GO, Builder, 2, &[]).await;
}

#[tokio::test]
async fn go_named_zero_without_colon() {
    check(
        "go-zero-only",
        "go",
        "User",
        &GO.replace("x:y", "xy"),
        Factory,
        2,
        &[],
    )
    .await;
}

#[tokio::test]
async fn retained_fixed_factories_and_python_saved_fields() {
    check("js-factory-fixed", "mjs", "Bag", "export class Bag {\n constructor(value) { this.value = value; }\n}\nconst bag = new Bag(7);\nconsole.log(bag.value);\n", Factory, 1, &[]).await;
    check("js-factory-empty", "mjs", "Bag", "export class Bag {\n constructor() {}\n}\nconst bag = new Bag();\nconsole.log(bag instanceof Bag);\n", Factory, 1, &[]).await;
    check("python-saved-field", "py", "Person", "class Person:\n    def __init__(self, name: str):\n        self.name = name\nname = 'global'\nperson = Person('Ada')\nprint(person.name)\n", Builder, 1, &["return Person(name=self._name)"]).await;
}
