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

const OWNER: &str = r#"struct Config { value: i32 }
mod inner {
    #[derive(Default)] pub struct Config { pub value: i32 }
    impl Config {
        pub fn new(value: i32) -> Self { let mut result = Self::default(); result.value = value + 10; result }
        pub fn make() -> Self { Self { value: 2 } }
    }
}
impl Config { fn make() -> Self { Self { value: 3 } } }
fn main() {
    let outer = Config { value: 1 };
    println!("{} {} {}", outer.value, inner::Config::make().value, Config::make().value);
}
"#;

const OPERATORS: &str = r#"struct Pair { first: bool, second: bool }
fn main() {
    let a = Pair { first: 2 > 1, second: true };
    let b = Pair { first: 1 < 2, second: 8 >> 1 == 4 };
    let c = Pair { first: 1 << 2 == 4, second: 3 >= 2 };
    println!("{} {} {} {} {} {}", a.first, a.second, b.first, b.second, c.first, c.second);
}
"#;

#[tokio::test]
async fn aggregate_owner_factory() {
    check(
        "aggregate-owner-factory",
        "rs",
        "Config",
        OWNER,
        Factory,
        2,
        &["Self { value: 2 }", "result.value = value + 10"],
    )
    .await;
}

#[tokio::test]
async fn aggregate_owner_builder() {
    check(
        "aggregate-owner-builder",
        "rs",
        "Config",
        OWNER,
        Builder,
        2,
        &["Self { value: 2 }", "result.value = value + 10"],
    )
    .await;
}

#[tokio::test]
async fn aggregate_operators_factory() {
    check(
        "aggregate-operators-factory",
        "rs",
        "Pair",
        OPERATORS,
        Factory,
        3,
        &[],
    )
    .await;
}

#[tokio::test]
async fn aggregate_operators_builder() {
    check(
        "aggregate-operators-builder",
        "rs",
        "Pair",
        OPERATORS,
        Builder,
        3,
        &[],
    )
    .await;
}

#[tokio::test]
async fn aggregate_python_storage() {
    let source = "class Config:\n    def __init__(self, provided: bool = False, provided_1 = 'default'):\n        self.provided = provided\n        self.other = provided_1\na = Config(True)\nb = Config()\nc = Config(None, None)\nprint(a.provided, a.other, b.provided, b.other, c.provided, c.other)\n";
    check(
        "aggregate-python-storage",
        "py",
        "Config",
        source,
        Builder,
        3,
        &[],
    )
    .await;
}

#[tokio::test]
async fn aggregate_javascript_helper() {
    let source = "export class Bag {\n constructor(...withArguments) { this.items = withArguments; }\n}\nconst bag = new Bag(1, 2);\nconsole.log(JSON.stringify(bag.items));\n";
    check(
        "aggregate-javascript-helper",
        "mjs",
        "Bag",
        source,
        Builder,
        1,
        &[],
    )
    .await;
}

#[tokio::test]
async fn aggregate_javascript_member_names() {
    let source = "export class Bag {\n constructor(withArguments, withArguments_1, ..._arguments) { this.items = [withArguments, withArguments_1, ..._arguments]; }\n}\nconst values = [1, 2, 3, 4];\nconst a = new Bag(...values);\nconst b = new Bag(5, 6, 7);\nconsole.log(JSON.stringify(a.items), JSON.stringify(b.items));\n";
    let generated = check(
        "aggregate-javascript-member-names",
        "mjs",
        "Bag",
        source,
        Builder,
        2,
        &[],
    )
    .await;
    // Also exercise the public saved-field setters rather than just forwarding.
    let manual = format!(
        "{generated}\nconsole.log(JSON.stringify(new BagBuilder().withArguments(1).withArguments_1(2)._arguments([3,4]).build().items));\n"
    );
    let ws = prod_code_testkit::Workspace::new(&[("main.mjs", &manual)]);
    let result = super::support::run(&ws.root(), &["node", "main.mjs"]);
    super::support::record(
        "aggregate-javascript-manual-setters",
        &serde_json::json!({
            "program":super::support::bytes(&ws.root(), &manual), "runtime":result
        }),
    );
    assert_eq!(result["exit"], 0, "{result}");
    assert_eq!(result["stdout"], "[1,2,3,4] [5,6,7]\n[1,2,3,4]\n");
}

#[tokio::test]
async fn aggregate_rust_expression_controls() {
    let source = r####"struct Pair { first: usize, second: &'static str }
fn choose<T, const N: usize>(_: T) -> usize { N }
fn main() {
    let π = Pair { first: /* outer /* inner */ kept */ choose::<(u8, u8), { 8 >> 1 }>((1, 2)), second: r###"é, : < >"### };
    println!("{} {}", π.first, π.second);
}
"####;
    for mode in [Factory, Builder] {
        check(
            &format!("aggregate-expression-controls-{mode}"),
            "rs",
            "Pair",
            source,
            mode,
            1,
            &[
                "/* outer /* inner */ kept */",
                "choose::<(u8, u8), { 8 >> 1 }>((1, 2))",
                "r###\"é, : < >\"###",
            ],
        )
        .await;
    }
}

#[tokio::test]
async fn aggregate_owner_qualified_and_local() {
    let source = r#"struct Config { value: i32 }
mod child {
    pub struct Config { pub value: i32 }
    impl Config { pub fn other() -> Self { Self { value: 9 } } }
    impl super::Config { pub fn selected() -> Self { Self { value: 4 } } }
}
mod imported {
    use super::{Config as Selected};
    impl Selected { pub fn imported() -> Self { Self { value: 5 } } }
}
impl crate::Config {
    fn make() -> Self {
        struct Config { value: i32 }
        impl Config { fn other() -> Self { Self { value: 8 } } }
        let other = Config { value: 7 };
        println!("{} {}", other.value, Config::other().value);
        Self { value: 3 }
    }
}
fn main() { println!("{} {} {} {}", Config::make().value, Config::selected().value, child::Config::other().value, Config::imported().value); }
"#;
    for mode in [Factory, Builder] {
        check(
            &format!("aggregate-owner-qualified-{mode}"),
            "rs",
            "Config",
            source,
            mode,
            3,
            &[
                "Self { value: 9 }",
                "Self { value: 8 }",
                "Config { value: 7 }",
            ],
        )
        .await;
    }
}

#[tokio::test]
async fn aggregate_parse_failure_is_refused() {
    let source = "struct Pair { first: bool, second: bool }\nfn main() { let p = Pair { first: 2 >, second: true }; }\n";
    for mode in [Factory, Builder] {
        let ws = prod_code_testkit::Workspace::new(&[
            ("main.rs", source),
            (
                "Cargo.toml",
                "[package]\nname='fixture'\nversion='0.1.0'\nedition='2021'\n",
            ),
        ]);
        let result = super::support::transform(&ws, "rs", "Pair", mode, true).await;
        let after = std::fs::read_to_string(ws.root().join("main.rs")).unwrap();
        super::support::record(
            &format!("aggregate-parse-refusal-{mode}"),
            &serde_json::json!({
                "input":super::support::bytes(&ws.root(), source),
                "output":super::support::bytes(&ws.root(), &after),
                "error":result.as_ref().err().map(|e| format!("{e:#}"))
            }),
        );
        assert!(
            format!("{:#}", result.unwrap_err()).contains("cannot parse Rust constructor source")
        );
        assert_eq!(after, source);
    }
}

#[tokio::test]
async fn aggregate_consumer_editions() {
    for (edition, module) in [(2015, "async"), (2021, "gen")] {
        let source = format!(
            "struct Pair {{ first: bool, second: bool }}\nmod {module} {{ pub fn value() -> bool {{ true }} }}\nfn main() {{ let p = Pair {{ first: {module}::value(), second: 8 >> 1 == 4 }}; println!(\"{{}} {{}}\", p.first, p.second); }}\n"
        );
        for mode in [Factory, Builder] {
            super::support::check_with_edition(
                &format!("aggregate-edition-{edition}-{mode}"),
                "rs",
                "Pair",
                &source,
                mode,
                1,
                &[],
                edition,
            )
            .await;
        }
    }
}

#[tokio::test]
async fn aggregate_typescript_helper() {
    let source = "export class Bag {\n items: number[];\n constructor(withArguments: number, withArguments_1: number, ..._arguments: number[]) { this.items = [withArguments, withArguments_1, ..._arguments]; }\n}\nconst values: [number, number, ...number[]] = [1, 2, 3, 4];\nconst bag = new Bag(...values);\nconsole.log(JSON.stringify(bag.items));\n";
    check(
        "aggregate-typescript-helper",
        "ts",
        "Bag",
        source,
        Builder,
        1,
        &[],
    )
    .await;
}
