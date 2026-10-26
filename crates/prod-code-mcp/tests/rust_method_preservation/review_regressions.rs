/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::support::*;

#[tokio::test]
async fn recursive_destination_cannot_be_captured_by_local_imports() {
    let source = r#"struct Tax;
struct Order;
struct Decoy;
impl Order {
    fn f(n: u32) -> u32 {
        use crate::Decoy as Tax;
        let recur = Self::f;
        if n == 0 { 1 } else { recur(n - 1) + 1 }
    }
}
impl Decoy { fn f(_: u32) -> u32 { 99 } }
fn main() { println!("{}", Order::f(1)); }
"#;
    let ws = workspace(source);
    let remote = gateway(&ws, source, &[("f(n:", &["f;", "f(1)"])]).await;
    assert_eq!(run(source, "shadow recursion original"), "2\n");
    let plan = move_associated(&ws, remote, false, false).await.unwrap();
    untouched(&ws, source);
    assert_eq!(run(&plan.rewritten[0].1, "shadow recursion preview"), "2\n");
    let moved = move_associated(&ws, remote, true, false).await.unwrap();
    assert!(moved.applied);
    assert_eq!(moved.calls, 2);
    assert_eq!(
        run(&ws.read("src/main.rs"), "shadow recursion applied"),
        "2\n"
    );
}

#[tokio::test]
async fn macro_owner_tokens_keep_outer_self_in_associated_move() {
    let source = r#"struct Tax;
struct Order(u64);
impl Order {
    fn f() -> usize {
        macro_rules! size { (impl $t:ty) => { std::mem::size_of::<$t>() }; }
        let n = size!(impl Self);
        n
    }
}
fn main() { println!("{}", Order::f()); }
"#;
    let ws = workspace(source);
    let remote = gateway(&ws, source, &[("f() ->", &["f());"])]).await;
    assert_eq!(run(source, "macro associated original"), "8\n");
    let plan = move_associated(&ws, remote, false, false).await.unwrap();
    untouched(&ws, source);
    assert_eq!(run(&plan.rewritten[0].1, "macro associated preview"), "8\n");
    let moved = move_associated(&ws, remote, true, false).await.unwrap();
    assert!(moved.applied);
    assert_eq!(
        run(&ws.read("src/main.rs"), "macro associated applied"),
        "8\n"
    );
}

#[tokio::test]
async fn macro_owner_tokens_keep_outer_receiver_and_self_type() {
    let source = r#"struct Tax(u32);
struct Order(u64);
impl Order {
    fn f(&self, tax: &Tax) -> usize {
        macro_rules! size { (impl $t:ty) => { std::mem::size_of::<$t>() }; }
        let n = size!(impl Self) + self.0 as usize;
        let _ = tax;
        n
    }
}
fn main() { let o = Order(2); let t = Tax(3); println!("{}", o.f(&t)); }
"#;
    let ws = workspace(source);
    let remote = gateway(
        &ws,
        source,
        &[("tax: &Tax", &["tax;"]), ("f(&self", &["f(&t)"])],
    )
    .await;
    assert_eq!(run(source, "macro receiver original"), "10\n");
    let plan = move_receiver(&ws, remote, false, false).await.unwrap();
    untouched(&ws, source);
    assert_eq!(run(&plan.rewritten[0].1, "macro receiver preview"), "10\n");
    let moved = move_receiver(&ws, remote, true, false).await.unwrap();
    assert!(moved.applied);
    assert_eq!(
        run(&ws.read("src/main.rs"), "macro receiver applied"),
        "10\n"
    );
}

#[tokio::test]
async fn return_position_impl_trait_does_not_introduce_a_self_owner() {
    let source = r#"struct Tax;
struct Order(u64);
impl Order {
    fn f() -> /* impl Self */ impl Iterator<Item = usize> {
        std::iter::once(std::mem::size_of::<Self>())
    }
}
fn main() { println!("{}", Order::f().next().unwrap()); }
"#;
    let ws = workspace(source);
    let remote = gateway(&ws, source, &[("f() ->", &["f().next"])]).await;
    assert_eq!(run(source, "impl trait original"), "8\n");
    let plan = move_associated(&ws, remote, false, false).await.unwrap();
    untouched(&ws, source);
    assert_eq!(run(&plan.rewritten[0].1, "impl trait preview"), "8\n");
    let moved = move_associated(&ws, remote, true, false).await.unwrap();
    assert!(moved.applied);
    assert_eq!(run(&ws.read("src/main.rs"), "impl trait applied"), "8\n");
}

#[tokio::test]
async fn keyword_macro_arguments_respect_each_token_tree_delimiter() {
    for invocation in ["size!(impl Self)", "size![trait Self]", "size!{mod Self}"] {
        let source = format!(
            "struct Tax;\nstruct Order(u64);\nimpl Order {{\n    fn f() -> usize {{\n        macro_rules! size {{ ($tag:ident $t:ty) => {{ std::mem::size_of::<$t>() }}; }}\n        let n = {invocation}; n\n    }}\n}}\nfn main() {{ println!(\"{{}}\", Order::f()); }}\n"
        );
        let ws = workspace(&source);
        let remote = gateway(&ws, &source, &[("f() ->", &["f());"])]).await;
        assert_eq!(run(&source, "macro delimiters original"), "8\n");
        let moved = move_associated(&ws, remote, true, false).await.unwrap();
        assert!(moved.applied);
        assert_eq!(
            run(&ws.read("src/main.rs"), "macro delimiters applied"),
            "8\n"
        );
    }
}

#[tokio::test]
async fn ambiguous_macro_owner_declarations_refuse_without_writing_even_with_force() {
    for receiver in [false, true] {
        let params = if receiver { "&self, tax: &Tax" } else { "" };
        let call = if receiver { "o.f(&t)" } else { "Order::f()" };
        let source = format!(
            "struct Tax;\nstruct Order(u64);\nimpl Order {{\n    fn f({params}) -> usize {{\n        macro_rules! size {{ (impl $t:ty {{}}) => {{ std::mem::size_of::<$t>() }}; }}\n        let n = size!(impl Self {{}}); n\n    }}\n}}\nfn main() {{ let o = Order(2); let t = Tax; println!(\"{{}}\", {call}); }}\n"
        );
        assert_eq!(run(&source, "ambiguous macro original"), "8\n");
        let ws = workspace(&source);
        let remote = gateway(&ws, &source, &[]).await;
        for force in [false, true] {
            ws.write("src/main.rs", &source);
            let result = if receiver {
                move_receiver(&ws, remote, true, force).await
            } else {
                move_associated(&ws, remote, true, force).await
            };
            let err = result.unwrap_err().to_string();
            println!("ambiguous macro receiver={receiver} force={force}: {err}");
            assert!(err.contains("cannot establish Self ownership"), "{err}");
            untouched(&ws, &source);
        }
    }
}
