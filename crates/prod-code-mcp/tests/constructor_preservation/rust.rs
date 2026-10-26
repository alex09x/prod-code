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

const ORDER: &str = r#"struct Effect { id: i32 }
impl Clone for Effect {
    fn clone(&self) -> Self { println!("{}", self.id); Self { id: self.id } }
}
struct Pair {
    first: Effect,
    second: Effect,
}
fn main() {
    let a = Effect { id: 1 }; let b = Effect { id: 2 };
    let p = Pair { second: b.clone(), first: a.clone() };
    println!("{} {}", p.first.id, p.second.id);
}
"#;

#[tokio::test]
async fn effectful_methods_factory() {
    check("order-factory", "rs", "Pair", ORDER, Factory, 1, &[]).await;
}
#[tokio::test]
async fn effectful_methods_builder() {
    check("order-builder", "rs", "Pair", ORDER, Builder, 1, &[]).await;
}

const LEXICAL: &str = r####"struct Config {
    value: i32,
}
/* outer /* inner */ Config { value: 3 } */
fn main() {
    let sample = r###"Config { value: 4 }"###;
    let actual = Config { value: 5 };
    println!("{} {}", sample, actual.value);
}
"####;
#[tokio::test]
async fn lexical_exclusion() {
    for mode in [Factory, Builder] {
        check(
            &format!("lexical-{mode}"),
            "rs",
            "Config",
            LEXICAL,
            mode,
            1,
            &[
                "/* outer /* inner */ Config { value: 3 } */",
                "r###\"Config { value: 4 }\"###",
            ],
        )
        .await;
    }
}

const OWNER: &str = r#"struct Config {
    value: i32,
}
impl Config {
    fn make() -> Self {
        struct Other { value: i32 }
        impl Other {
            fn make() -> Self { Self { value: 2 } }
        }
        println!("{}", Other::make().value);
        Self { value: 1 }
    }
}
fn main() { println!("{}", Config::make().value); }
"#;
#[tokio::test]
async fn nearest_self_owner() {
    for mode in [Factory, Builder] {
        check(
            &format!("owner-{mode}"),
            "rs",
            "Config",
            OWNER,
            mode,
            1,
            &["fn make() -> Self { Self { value: 2 } }"],
        )
        .await;
    }
    let inline = OWNER
        .replace(
            "impl Config {\n    fn make() -> Self {",
            "impl Config { fn make() -> Self {",
        )
        .replace(
            "        impl Other {\n            fn make() -> Self { Self { value: 2 } }\n        }",
            "        impl Other { fn make() -> Self { Self { value: 2 } } }",
        );
    for mode in [Factory, Builder] {
        check(
            &format!("owner-inline-{mode}"),
            "rs",
            "Config",
            &inline,
            mode,
            1,
            &["impl Other { fn make() -> Self { Self { value: 2 } } }"],
        )
        .await;
    }
}

const GENERIC: &str = r#"#[derive(Clone)]
struct NoDefault;
struct Item<'a, T: Clone = NoDefault, const N: usize = 2> {
    value: &'a T,
    array: [u8; N],
}
fn main() {
    let value = NoDefault;
    let item: Item<'_, NoDefault, 2> = Item { value: &value, array: [1, 2] };
    println!("{} {}", std::mem::size_of_val(item.value), item.array[1]);
}
"#;
#[tokio::test]
async fn generic_factory() {
    check("generic-factory", "rs", "Item", GENERIC, Factory, 1, &[]).await;
}
#[tokio::test]
async fn generic_builder() {
    check("generic-builder", "rs", "Item", GENERIC, Builder, 1, &[]).await;
}

#[tokio::test]
async fn conversion_methods_and_ordinary_calls() {
    for (method, implementation, ty, read) in [
        (
            "into",
            "impl From<Effect> for i32 { fn from(v: Effect) -> Self { println!(\"{}\", v.id); v.id } }",
            "i32",
            "p.first",
        ),
        (
            "to_owned",
            "impl ToOwned for Effect { type Owned = Effect; fn to_owned(&self) -> Effect { println!(\"{}\", self.id); Effect { id: self.id } } }",
            "Effect",
            "p.first.id",
        ),
        (
            "to_string",
            "impl ToString for Effect { fn to_string(&self) -> String { println!(\"{}\", self.id); self.id.to_string() } }",
            "String",
            "p.first",
        ),
    ] {
        let source = format!(
            "struct Effect {{ id: i32 }}\n{implementation}\nstruct Pair {{\n first: {ty},\n second: {ty},\n}}\nfn main() {{ let a = Effect {{ id: 1 }}; let b = Effect {{ id: 2 }}; let p = Pair {{ second: b.{method}(), first: a.{method}() }}; println!(\"{{}}\", {read}); }}\n"
        );
        for mode in [Factory, Builder] {
            check(
                &format!("method-{method}-{mode}"),
                "rs",
                "Pair",
                &source,
                mode,
                1,
                &[],
            )
            .await;
        }
    }
    for (kind, literal) in [
        ("source", "first: tick(1), second: tick(2)"),
        ("reverse", "second: tick(2), first: tick(1)"),
        ("pure", "second: 2, first: 1"),
    ] {
        let source = format!(
            "struct Pair {{\n first: i32,\n second: i32,\n}}\nfn tick(n: i32) -> i32 {{ println!(\"{{n}}\"); n }}\nfn main() {{ let p = Pair {{ {literal} }}; println!(\"{{}} {{}}\", p.first, p.second); }}\n"
        );
        for mode in [Factory, Builder] {
            check(
                &format!("ordinary-{kind}-{mode}"),
                "rs",
                "Pair",
                &source,
                mode,
                1,
                &[],
            )
            .await;
        }
    }
}

#[tokio::test]
async fn nested_generic_bounds_and_where_clause() {
    let source = "struct Item<'a, F: for<'b> Fn(&'b str) -> usize> where F: Clone {\n    value: &'a F,\n    pair: (u8, u8),\n}\nfn main() {\n    let f = |s: &str| s.len();\n    let item = Item { value: &f, pair: (1, 2) };\n    println!(\"{} {}\", (item.value)(\"abc\"), item.pair.1);\n}\n";
    for mode in [Factory, Builder] {
        check(
            &format!("nested-generics-{mode}"),
            "rs",
            "Item",
            source,
            mode,
            1,
            &[],
        )
        .await;
    }
}

#[tokio::test]
async fn explicit_generic_arguments_and_selected_self() {
    let source = "struct Item<T, const N: usize> {\n    value: std::marker::PhantomData<T>,\n    data: [u8; N],\n}\nimpl<T, const N: usize> Item<T, N> {\n    fn make() -> Self {\n        Self { value: std::marker::PhantomData, data: [1; N] }\n    }\n}\nfn main() {\n    let a = Item::<u8, 2> { value: std::marker::PhantomData, data: [2, 3] };\n    let b = Item::<u8, 3>::make();\n    println!(\"{} {}\", a.data[1], b.data[2]);\n}\n";
    for mode in [Factory, Builder] {
        check(
            &format!("explicit-generics-{mode}"),
            "rs",
            "Item",
            source,
            mode,
            2,
            &[],
        )
        .await;
    }
}
